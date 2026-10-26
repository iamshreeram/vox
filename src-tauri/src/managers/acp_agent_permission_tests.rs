//! Integration tests for ACP permission handling and cancel (DESIGN.md Revision 3,
//! M2 "Turn timing + cancel (S3)", C2, C3, C4, C12). TDD red until the S3 step
//! wires `acp_permission_broker` and `acp_run` into `acp_agent.rs`.
//!
//! The fake agent is a python3 script driven by a JSON scenario. It reads stdin on
//! a separate thread, so it keeps answering other messages while an approval is
//! pending, exactly like a real agent. Every JSON-RPC message it receives (and every
//! message it sends) is appended to `log.jsonl`, so tests can assert what the agent
//! actually saw. All waits are capped polls; the only fixed sleeps are documented
//! settle delays used to prove a negative ("this never reached the agent").

#[cfg(unix)]
mod unix_tests {
    use crate::managers::acp_agent::*;
    use crate::managers::acp_permission::{PermissionMode, UserChoice};
    use crate::managers::acp_permission_broker::{
        PermissionBroker, PermissionPrompter, PermissionView,
    };
    use crate::managers::acp_run::RunHandle;
    use serde_json::{json, Value};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    const FAKE_PY: &str = r##"
import sys, os, json, threading, queue, time

SCN_PATH = sys.argv[1]
with open(SCN_PATH) as f:
    SCN = json.load(f)
DIR = os.path.dirname(os.path.abspath(SCN_PATH))
LOG = os.path.join(DIR, "log.jsonl")
SEQ = os.path.join(DIR, "seq")
WLOCK = threading.Lock()
LOGLOCK = threading.Lock()
CV = threading.Condition()
RESPONSES = {}
REQS = queue.Queue()
CANCEL = threading.Event()
COUNT = [0]

def log(ev):
    with LOGLOCK:
        with open(LOG, "a") as f:
            f.write(json.dumps(ev) + "\n")

def send(obj):
    data = (json.dumps(obj) + "\n").encode()
    with WLOCK:
        sys.stdout.buffer.write(data)
        sys.stdout.buffer.flush()

def key(v):
    return json.dumps(v, sort_keys=True)

def wait_until(pred, timeout):
    end = time.time() + timeout
    while time.time() < end:
        if pred():
            return True
        time.sleep(0.01)
    return pred()

def summ(m):
    params = m.get("params") or {}
    if m.get("method") == "session/prompt":
        text = (params.get("prompt") or [{}])[0].get("text", "")
        params = {"sessionId": params.get("sessionId"), "len": len(text)}
    return {"event": "recv", "method": m.get("method"), "id": m.get("id"),
            "params": params, "result": m.get("result"), "error": m.get("error")}

def reader():
    for raw in sys.stdin.buffer:
        try:
            m = json.loads(raw)
        except Exception:
            continue
        if not isinstance(m, dict):
            continue
        log(summ(m))
        method = m.get("method")
        if method == "session/cancel":
            CANCEL.set()
        elif method is None and "id" in m:
            with CV:
                RESPONSES.setdefault(key(m["id"]), m)
                CV.notify_all()
        elif method:
            REQS.put(m)
    log({"event": "eof"})
    os._exit(0)

def chunk(sid, text):
    send({"jsonrpc": "2.0", "method": "session/update",
          "params": {"sessionId": sid, "update": {"sessionUpdate": "agent_message_chunk",
                     "content": {"type": "text", "text": text}}}})
    log({"event": "sent", "kind": "chunk", "text": text})

def do_init(m):
    d = SCN.get("init_delay_ms", 0)
    if d:
        time.sleep(d / 1000.0)
    send({"jsonrpc": "2.0", "id": m["id"], "result": {
        "protocolVersion": 1, "agentCapabilities": {},
        "agentInfo": {"name": "perm-fake", "version": "0"}, "authMethods": []}})

def do_new(m):
    COUNT[0] += 1
    sid = "sess-%d-%d" % (os.getpid(), COUNT[0])
    send({"jsonrpc": "2.0", "id": m["id"], "result": {"sessionId": sid, "configOptions": []}})

def next_index():
    n = 0
    if os.path.exists(SEQ):
        with open(SEQ) as f:
            n = int(f.read().strip() or 0)
    with open(SEQ, "w") as f:
        f.write(str(n + 1))
    return n

def wait_response(rid, timeout):
    end = time.time() + timeout
    with CV:
        while key(rid) not in RESPONSES and not CANCEL.is_set():
            left = end - time.time()
            if left <= 0:
                break
            CV.wait(min(left, 0.05))

def run_step(st, ctx):
    op = st["op"]
    if op == "chunk":
        chunk(ctx["sid"], st["text"])
    elif op == "touch":
        open(os.path.join(DIR, st["name"]), "a").close()
    elif op == "wait_file":
        wait_until(lambda: os.path.exists(os.path.join(DIR, st["name"])), st.get("timeout", 10))
    elif op == "sleep":
        time.sleep(st["seconds"])
    elif op == "request":
        params = dict(st["params"])
        if params.get("sessionId") == "{session}":
            params["sessionId"] = ctx["sid"]
        send({"jsonrpc": "2.0", "id": st["id"], "method": "session/request_permission",
              "params": params})
        log({"event": "sent", "kind": "permission_request", "id": st["id"]})
    elif op == "await":
        wait_response(st["id"], st.get("timeout", 10))
    elif op == "wait_cancel":
        CANCEL.wait(st.get("timeout", 10))
    elif op == "finish":
        stop = st.get("stop", "end_turn")
        if CANCEL.is_set():
            if SCN.get("on_cancel", "cancelled") == "ignore":
                time.sleep(3600)
            stop = "cancelled"
        if stop == "none":
            send({"jsonrpc": "2.0", "id": ctx["rid"], "result": {}})
        else:
            send({"jsonrpc": "2.0", "id": ctx["rid"], "result": {"stopReason": stop}})
    else:
        raise ValueError("unknown step " + op)

def do_prompt(m):
    sid = m["params"]["sessionId"]
    idx = next_index()
    prompts = SCN["prompts"]
    steps = prompts[idx] if idx < len(prompts) else prompts[-1]
    ctx = {"sid": sid, "rid": m["id"]}
    for st in steps:
        run_step(st, ctx)

def main():
    threading.Thread(target=reader, daemon=True).start()
    log({"event": "start", "pid": os.getpid()})
    while True:
        m = REQS.get()
        method = m.get("method")
        if method == "initialize":
            do_init(m)
        elif method == "session/new":
            do_new(m)
        elif method == "session/prompt":
            do_prompt(m)
        elif method == "session/close":
            send({"jsonrpc": "2.0", "id": m.get("id"), "result": {}})
        elif "id" in m:
            send({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32601, "message": "method not found"}})

main()
"##;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn python() -> &'static PathBuf {
        static PY: OnceLock<PathBuf> = OnceLock::new();
        PY.get_or_init(|| {
            let out = std::process::Command::new("python3")
                .args(["-c", "import sys; print(sys.executable)"])
                .output()
                .expect("PREREQUISITE: python3 must be installed and on PATH for the ACP fake-agent tests");
            assert!(out.status.success(), "PREREQUISITE: python3 failed to run");
            PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())
        })
    }

    /// One scripted fake agent. `prompts[n]` is the step list for the n-th
    /// `session/prompt` across ALL child processes of this test (a persistent counter),
    /// so a respawned child continues the script.
    struct Fake {
        dir: PathBuf,
    }

    impl Fake {
        fn new(tag: &str, prompts: Vec<Vec<Value>>, extra: Value) -> Fake {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("vox-acp-perm-{tag}-{}-{n}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("fake_agent.py"), FAKE_PY).unwrap();
            let mut scn = extra;
            scn["prompts"] = Value::Array(prompts.into_iter().map(Value::Array).collect());
            fs::write(dir.join("scn.json"), scn.to_string()).unwrap();
            Fake { dir }
        }

        fn plain(tag: &str, prompts: Vec<Vec<Value>>) -> Fake {
            Fake::new(tag, prompts, json!({}))
        }

        fn cfg(&self, turn: Duration, grace: Duration) -> AcpAgentConfig {
            AcpAgentConfig {
                command: python().clone(),
                args: vec![
                    self.dir
                        .join("fake_agent.py")
                        .to_string_lossy()
                        .into_owned(),
                    self.dir.join("scn.json").to_string_lossy().into_owned(),
                ],
                cwd: self.dir.clone(),
                turn_timeout: turn,
                handshake_timeout: Duration::from_secs(10),
                cancel_grace: grace,
            }
        }

        fn events(&self) -> Vec<Value> {
            fs::read_to_string(self.dir.join("log.jsonl"))
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        }

        fn recv(&self, method: &str) -> Vec<Value> {
            self.events()
                .into_iter()
                .filter(|e| e["event"] == "recv" && e["method"] == method)
                .collect()
        }

        /// The first JSON-RPC response the agent received for request `id` (typed match).
        fn response(&self, id: &Value) -> Option<Value> {
            self.events()
                .into_iter()
                .find(|e| e["event"] == "recv" && e["method"].is_null() && e["id"] == *id)
        }

        /// The `outcome` object of the response for `id`, or Null when none arrived.
        fn outcome(&self, id: &Value) -> Value {
            self.response(id)
                .map(|e| e["result"]["outcome"].clone())
                .unwrap_or(Value::Null)
        }

        fn starts(&self) -> usize {
            self.events()
                .iter()
                .filter(|e| e["event"] == "start")
                .count()
        }

        fn touched(&self, name: &str) -> bool {
            self.dir.join(name).exists()
        }

        fn release(&self, name: &str) {
            fs::write(self.dir.join(name), b"go").unwrap();
        }

        fn pids(&self) -> Vec<i32> {
            self.events()
                .iter()
                .filter(|e| e["event"] == "start")
                .filter_map(|e| e["pid"].as_i64().map(|p| p as i32))
                .collect()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            // Best-effort cleanup of this test's own children (guards against pid reuse).
            let marker = self.dir.to_string_lossy().into_owned();
            for pid in self.pids() {
                let ps = std::process::Command::new("ps")
                    .args(["-p", &pid.to_string(), "-o", "command="])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).contains(&marker))
                    .unwrap_or(false);
                if ps {
                    // SAFETY: signals only a child this test spawned and just verified.
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    // ---------------------------------------------------------------- step builders

    fn chunk(text: &str) -> Value {
        json!({"op": "chunk", "text": text})
    }
    fn touch(name: &str) -> Value {
        json!({"op": "touch", "name": name})
    }
    fn wait_file(name: &str) -> Value {
        json!({"op": "wait_file", "name": name, "timeout": 10})
    }
    fn wait_cancel() -> Value {
        json!({"op": "wait_cancel", "timeout": 10})
    }
    fn finish(stop: &str) -> Value {
        json!({"op": "finish", "stop": stop})
    }
    fn await_resp(id: Value) -> Value {
        json!({"op": "await", "id": id, "timeout": 10})
    }
    /// A permission request from the agent, sent in the prompt's own session.
    fn ask(id: Value, kind: &str, raw_input: Option<Value>) -> Value {
        ask_in(id, kind, raw_input, "{session}")
    }
    fn ask_in(id: Value, kind: &str, raw_input: Option<Value>, session: &str) -> Value {
        let mut tool = json!({"toolCallId": "tc-1", "title": "Do a thing", "kind": kind});
        if let Some(raw) = raw_input {
            tool["rawInput"] = raw;
        }
        json!({"op": "request", "id": id, "params": {
        "sessionId": session,
        "toolCall": tool,
        "options": [
            {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "reject", "name": "Reject once", "kind": "reject_once"}
        ]}})
    }
    /// `{"outcome":{"outcome":"selected","optionId":id}}`
    fn selected(option: &str) -> Value {
        json!({"outcome": "selected", "optionId": option})
    }
    fn cancelled() -> Value {
        json!({"outcome": "cancelled"})
    }

    // ---------------------------------------------------------------- Rust-side fakes

    /// Records what the broker showed and what was closed. Never answers anything.
    #[derive(Default)]
    struct TestPrompter {
        seen: Mutex<Vec<PermissionView>>,
        closed: Mutex<Vec<String>>,
    }

    impl TestPrompter {
        fn views(&self) -> Vec<PermissionView> {
            self.seen.lock().unwrap().clone()
        }
        fn closed(&self) -> Vec<String> {
            self.closed.lock().unwrap().clone()
        }
    }

    impl PermissionPrompter for TestPrompter {
        fn present(&self, view: &PermissionView) {
            self.seen.lock().unwrap().push(view.clone());
        }
        fn close(&self, token: &str) {
            self.closed.lock().unwrap().push(token.to_string());
        }
    }

    /// Answers every presented prompt, in order, after `delay`. Stops on drop.
    struct Responder {
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    fn respond(
        m: &Arc<AcpAgentManager>,
        p: &Arc<TestPrompter>,
        choice: UserChoice,
        delay: Duration,
    ) -> Responder {
        let stop = Arc::new(AtomicBool::new(false));
        let (m, p, s) = (m.clone(), p.clone(), stop.clone());
        let handle = thread::spawn(move || {
            let mut next = 0;
            while !s.load(Ordering::SeqCst) {
                let views = p.views();
                if next < views.len() {
                    thread::sleep(delay);
                    if let Some(broker) = broker_of(&m) {
                        broker.resolve(&views[next].token, choice);
                    }
                    next += 1;
                } else {
                    thread::sleep(Duration::from_millis(5));
                }
            }
        });
        Responder {
            stop,
            handle: Some(handle),
        }
    }

    impl Drop for Responder {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    // ---------------------------------------------------------------- manager helpers

    /// The live broker of `m`, typed so tests can call `resolve` on it.
    fn broker_of(m: &AcpAgentManager) -> Option<Arc<PermissionBroker>> {
        m.permission_broker()
    }

    /// A manager with permissions configured (AskWrites/AskAll/Deny plus an approval timeout).
    fn perm_manager(
        p: &Arc<TestPrompter>,
        mode: PermissionMode,
        approval: Duration,
    ) -> Arc<AcpAgentManager> {
        let settings = Arc::new(move || PermissionSettings {
            mode,
            approval_timeout: approval,
        });
        Arc::new(AcpAgentManager::new().with_permissions(p.clone(), settings))
    }

    /// Polls `f` every 10 ms for at most `secs` seconds.
    fn wait_for(secs: u64, mut f: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        f()
    }

    fn opts() -> PromptOptions {
        PromptOptions {
            reuse_session: true,
            expected_epoch: None,
        }
    }

    fn spawn_prompt(
        m: &Arc<AcpAgentManager>,
        cfg: AcpAgentConfig,
        text: &'static str,
        reuse: bool,
    ) -> JoinHandle<Result<AcpReply, AcpAgentError>> {
        let m = m.clone();
        thread::spawn(move || {
            m.prompt(
                &cfg,
                text,
                PromptOptions {
                    reuse_session: reuse,
                    expected_epoch: None,
                },
            )
        })
    }

    fn spawn_with_handle(
        m: &Arc<AcpAgentManager>,
        cfg: AcpAgentConfig,
        text: &'static str,
        h: RunHandle,
    ) -> JoinHandle<Result<AcpReply, AcpAgentError>> {
        let m = m.clone();
        thread::spawn(move || m.prompt_with_handle(&cfg, text, opts(), &h))
    }

    // ---------------------------------------------------------------- policy (AskWrites / AskAll / Deny)

    #[test]
    fn perm_read_auto_allowed_selects_allow_once() {
        let f = Fake::plain(
            "read",
            vec![vec![
                chunk("done"),
                ask(json!("r1"), "read", None),
                await_resp(json!("r1")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        assert_eq!(f.outcome(&json!("r1")), selected("allow"));
        assert!(
            p.views().is_empty(),
            "auto-allowed reads must never be shown"
        );
    }

    #[test]
    fn perm_search_and_think_auto_allowed() {
        let f = Fake::plain(
            "searchthink",
            vec![vec![
                chunk("done"),
                ask(json!("s1"), "search", None),
                await_resp(json!("s1")),
                ask(json!("t1"), "think", None),
                await_resp(json!("t1")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        assert_eq!(f.outcome(&json!("s1")), selected("allow"));
        assert_eq!(f.outcome(&json!("t1")), selected("allow"));
        assert!(p.views().is_empty());
    }

    #[test]
    fn perm_read_with_contradicting_raw_input_asks() {
        let raw = json!({"command": "rm -rf ~/x"});
        let f = Fake::plain(
            "contra",
            vec![vec![
                chunk("done"),
                ask(json!("c1"), "read", Some(raw)),
                await_resp(json!("c1")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        let _answers = respond(&m, &p, UserChoice::Reject, Duration::ZERO);
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        assert_eq!(p.views().len(), 1, "a read carrying a command must ask");
        assert_eq!(f.outcome(&json!("c1")), selected("reject"));
    }

    #[test]
    fn perm_edit_asks_and_allow_selects_allow_once() {
        let f = Fake::plain(
            "editallow",
            vec![vec![
                chunk("done"),
                ask(json!("e1"), "edit", None),
                await_resp(json!("e1")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        let _answers = respond(&m, &p, UserChoice::Allow, Duration::ZERO);
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        assert_eq!(f.outcome(&json!("e1")), selected("allow"));
        let views = p.views();
        assert_eq!(views.len(), 1);
        assert_eq!(
            p.closed(),
            vec![views[0].token.clone()],
            "closed exactly once"
        );
    }

    #[test]
    fn perm_edit_reject_selects_reject_once() {
        let f = Fake::plain(
            "editreject",
            vec![vec![
                chunk("done"),
                ask(json!("e2"), "edit", None),
                await_resp(json!("e2")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        let _answers = respond(&m, &p, UserChoice::Reject, Duration::ZERO);
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        assert_eq!(f.outcome(&json!("e2")), selected("reject"));
        assert_eq!(p.views().len(), 1);
    }

    #[test]
    fn perm_timeout_selects_reject_once() {
        let f = Fake::plain(
            "timeout",
            vec![vec![
                chunk("done"),
                ask(json!("x1"), "edit", None),
                await_resp(json!("x1")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        // Nobody answers: the 1 s approval timeout must reject.
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(1));
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        assert_eq!(f.outcome(&json!("x1")), selected("reject"));
        assert_eq!(p.views().len(), 1);
        assert_eq!(p.closed().len(), 1);
    }

    #[test]
    fn perm_no_permissions_configured_cancels_like_before() {
        let f = Fake::plain(
            "noperm",
            vec![vec![
                chunk("done"),
                ask(json!("n1"), "edit", None),
                await_resp(json!("n1")),
                finish("end_turn"),
            ]],
        );
        let m = Arc::new(AcpAgentManager::new());
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        assert!(m.permission_broker().is_none());
        assert_eq!(f.outcome(&json!("n1")), cancelled());
    }

    #[test]
    fn perm_deny_mode_never_asks() {
        let f = Fake::plain(
            "deny",
            vec![vec![
                chunk("done"),
                ask(json!("d1"), "edit", None),
                await_resp(json!("d1")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::Deny, Duration::from_secs(60));
        // If the prompter were asked, this responder would allow it.
        let _answers = respond(&m, &p, UserChoice::Allow, Duration::ZERO);
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        let outcome = f.outcome(&json!("d1"));
        assert!(
            outcome == selected("reject") || outcome == cancelled(),
            "deny must refuse without asking, got {outcome}"
        );
        assert!(p.views().is_empty(), "deny mode must never show a prompt");
    }

    #[test]
    fn perm_ask_all_asks_even_for_read() {
        let f = Fake::plain(
            "askall",
            vec![vec![
                chunk("done"),
                ask(json!("a1"), "read", None),
                await_resp(json!("a1")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskAll, Duration::from_secs(60));
        let _answers = respond(&m, &p, UserChoice::Allow, Duration::ZERO);
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        assert_eq!(f.outcome(&json!("a1")), selected("allow"));
        assert_eq!(p.views().len(), 1, "ask-all must show even a read");
    }

    // ---------------------------------------------------------------- reader and turn timing

    #[test]
    fn perm_reader_not_blocked_while_approval_pending() {
        // A (chunk) -> request 1 -> MIDDLE (chunk) -> request 2. The agent only reaches
        // request 2 if the reader kept processing while request 1 awaits approval.
        let f = Fake::plain(
            "reader",
            vec![vec![
                chunk("A"),
                ask(json!(1), "edit", None),
                chunk("MIDDLE"),
                ask(json!(2), "edit", None),
                await_resp(json!(1)),
                await_resp(json!(2)),
                chunk("B"),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        let broker = m
            .permission_broker()
            .expect("broker after with_permissions");
        let h = spawn_prompt(
            &m,
            f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
            "hi",
            true,
        );
        assert!(
            wait_for(4, || broker.pending_len() == 2),
            "reader blocked: request 2 never reached the broker while request 1 was pending"
        );
        assert_eq!(p.views().len(), 1, "only one prompt is shown at a time");
        assert!(broker.resolve(&p.views()[0].token, UserChoice::Allow));
        assert!(wait_for(4, || p.views().len() == 2));
        assert!(broker.resolve(&p.views()[1].token, UserChoice::Allow));
        let r = join_within(h, 5).unwrap();
        assert_eq!(r.text, "AMIDDLEB");
        assert_eq!(f.outcome(&json!(1)), selected("allow"));
        assert_eq!(f.outcome(&json!(2)), selected("allow"));
    }

    #[test]
    fn perm_approval_crossing_original_turn_deadline_still_succeeds() {
        let f = Fake::plain(
            "cross",
            vec![vec![
                chunk("ok"),
                ask(json!("c1"), "edit", None),
                await_resp(json!("c1")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        // turn_timeout is 1 s; the approval lands at about 1.4 s.
        let _answers = respond(&m, &p, UserChoice::Allow, Duration::from_millis(1400));
        let started = Instant::now();
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(1400));
        assert_eq!(r.text, "ok");
        assert_eq!(f.outcome(&json!("c1")), selected("allow"));
    }

    #[test]
    fn perm_never_resolved_ends_by_hard_cap() {
        let f = Fake::plain(
            "hardcap",
            vec![vec![
                chunk("x"),
                ask(json!("h1"), "edit", None),
                await_resp(json!("h1")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        // Approval timeout 60 s, turn_timeout 1 s: the 3x hard cap (3 s) must win.
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        let started = Instant::now();
        let out = m.prompt(
            &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
            "hi",
            opts(),
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(4500),
            "hung for {elapsed:?}"
        );
        match out {
            Ok(_) => {}
            Err(AcpAgentError::Timeout) => {}
            Err(e) => panic!("unexpected error: {e}"),
        }
        assert_eq!(f.outcome(&json!("h1")), selected("reject"));
        assert_eq!(p.closed().len(), 1);
    }

    // ---------------------------------------------------------------- ids, flood, routing

    #[test]
    fn perm_rpc_id_types_echoed() {
        let f = Fake::plain(
            "ids",
            vec![vec![
                chunk("done"),
                ask(json!(41), "edit", None),
                await_resp(json!(41)),
                ask(json!("1"), "edit", None),
                await_resp(json!("1")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        let _answers = respond(&m, &p, UserChoice::Allow, Duration::ZERO);
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        let num = f.response(&json!(41)).expect("numeric id must be answered");
        assert!(num["id"].is_u64(), "numeric id changed type: {}", num["id"]);
        assert_eq!(num["result"]["outcome"]["optionId"], "allow");
        let text = f.response(&json!("1")).expect("string id must be answered");
        assert!(
            text["id"].is_string(),
            "string id changed type: {}",
            text["id"]
        );
        assert_eq!(text["result"]["outcome"]["optionId"], "allow");
    }

    #[test]
    fn perm_flood_beyond_32_cancelled_without_prompting() {
        // Sequential requests: each is answered before the next, so no queue overflow.
        let mut steps = vec![chunk("done")];
        for i in 1..=40 {
            steps.push(ask(json!(i), "edit", None));
            steps.push(await_resp(json!(i)));
        }
        steps.push(finish("end_turn"));
        let f = Fake::plain("flood", vec![steps]);
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        let _answers = respond(&m, &p, UserChoice::Allow, Duration::ZERO);
        // turn_timeout 2 s (not 1 s): 40 sequential round trips must fit comfortably.
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(2), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        assert_eq!(
            p.views().len(),
            32,
            "only the first 32 requests may be shown"
        );
        for i in 1..=32 {
            assert_eq!(f.outcome(&json!(i)), selected("allow"), "request {i}");
        }
        for i in 33..=40 {
            assert_eq!(f.outcome(&json!(i)), cancelled(), "request {i}");
        }
    }

    #[test]
    fn perm_wrong_session_cancelled_never_prompted() {
        let f = Fake::plain(
            "wrongsess",
            vec![vec![
                chunk("done"),
                ask_in(json!("w1"), "edit", None, "some-other-session"),
                await_resp(json!("w1")),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        // Would allow if it were ever asked.
        let _answers = respond(&m, &p, UserChoice::Allow, Duration::ZERO);
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        assert_eq!(f.outcome(&json!("w1")), cancelled());
        assert!(
            p.views().is_empty(),
            "a foreign session must never be shown"
        );
    }

    #[test]
    fn perm_request_after_turn_on_live_child_cancelled() {
        let f = Fake::plain(
            "late",
            vec![vec![
                chunk("done"),
                finish("end_turn"),
                ask(json!("l1"), "edit", None),
                await_resp(json!("l1")),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        let _answers = respond(&m, &p, UserChoice::Allow, Duration::ZERO);
        let r = m
            .prompt(
                &f.cfg(Duration::from_secs(1), Duration::from_secs(1)),
                "hi",
                opts(),
            )
            .unwrap();
        assert_eq!(r.text, "done");
        assert!(
            wait_for(5, || f.response(&json!("l1")).is_some()),
            "the late request must still be answered"
        );
        assert_eq!(f.outcome(&json!("l1")), cancelled());
        assert!(
            p.views().is_empty(),
            "a request outside any turn must never be shown"
        );
    }

    /// Joins a prompt thread, failing (instead of hanging) if it does not return in time.
    fn join_within(
        t: JoinHandle<Result<AcpReply, AcpAgentError>>,
        secs: u64,
    ) -> Result<AcpReply, AcpAgentError> {
        assert!(
            wait_for(secs, || t.is_finished()),
            "prompt did not return within {secs}s"
        );
        t.join().expect("prompt thread panicked")
    }

    // ---------------------------------------------------------------- cancel

    #[test]
    fn cancel_running_turn_returns_cancelled_and_child_survives() {
        let f = Fake::plain(
            "cancel",
            vec![
                vec![touch("started"), wait_cancel(), finish("cancelled")],
                vec![chunk("again"), finish("end_turn")],
            ],
        );
        let m = Arc::new(AcpAgentManager::new());
        let cfg = f.cfg(Duration::from_secs(10), Duration::from_secs(2));
        let h = m.new_run_handle();
        let t = spawn_with_handle(&m, cfg.clone(), "first", h);
        assert!(wait_for(5, || f.touched("started")), "turn never started");
        assert!(m.cancel(&h), "cancelling the running turn must succeed");
        let out = join_within(t, 5);
        assert!(matches!(out, Err(AcpAgentError::Cancelled)), "{out:?}");
        let r = m.prompt(&cfg, "second", opts()).unwrap();
        assert_eq!(r.text, "again");
        assert_eq!(f.starts(), 1, "a cancel must not respawn the child");
    }

    #[test]
    fn cancel_with_reuse_session_false() {
        let f = Fake::plain(
            "cancelfresh",
            vec![
                vec![touch("started"), wait_cancel(), finish("cancelled")],
                vec![chunk("again"), finish("end_turn")],
            ],
        );
        let m = Arc::new(AcpAgentManager::new());
        let cfg = f.cfg(Duration::from_secs(10), Duration::from_secs(2));
        let h = m.new_run_handle();
        let (m2, cfg2) = (m.clone(), cfg.clone());
        let t = thread::spawn(move || {
            m2.prompt_with_handle(
                &cfg2,
                "first",
                PromptOptions {
                    reuse_session: false,
                    expected_epoch: None,
                },
                &h,
            )
        });
        assert!(wait_for(5, || f.touched("started")), "turn never started");
        assert!(m.cancel(&h));
        let out = join_within(t, 5);
        assert!(matches!(out, Err(AcpAgentError::Cancelled)), "{out:?}");
        let r = m
            .prompt(
                &cfg,
                "second",
                PromptOptions {
                    reuse_session: false,
                    expected_epoch: None,
                },
            )
            .unwrap();
        assert_eq!(r.text, "again");
        assert_eq!(f.starts(), 1);
    }

    #[test]
    fn cancel_during_handshake_never_sends_prompt() {
        // The agent answers initialize only after 1.5 s; the cancel lands before that.
        let f = Fake::new(
            "handshake",
            vec![vec![chunk("late"), finish("end_turn")]],
            json!({"init_delay_ms": 1500}),
        );
        let m = Arc::new(AcpAgentManager::new());
        let cfg = f.cfg(Duration::from_secs(10), Duration::from_secs(2));
        let h = m.new_run_handle();
        let t = spawn_with_handle(&m, cfg, "hi", h);
        assert!(
            wait_for(5, || !f.recv("initialize").is_empty()),
            "handshake never started"
        );
        let cancelled_at = Instant::now();
        m.cancel(&h);
        let out = join_within(t, 5);
        assert!(matches!(out, Err(AcpAgentError::Cancelled)), "{out:?}");
        assert!(
            cancelled_at.elapsed() < Duration::from_secs(1),
            "cancel during handshake must abort promptly"
        );
        // Settle: wait past the agent's delayed initialize reply, then look for a prompt.
        thread::sleep(Duration::from_millis(2000));
        assert!(
            f.recv("session/prompt").is_empty(),
            "session/prompt must never be sent"
        );
    }

    #[test]
    fn busy_caller_cannot_cancel_running_turn() {
        let f = Fake::plain(
            "busy",
            vec![vec![
                touch("started"),
                wait_file("gate"),
                chunk("first-done"),
                finish("end_turn"),
            ]],
        );
        let m = Arc::new(AcpAgentManager::new());
        let cfg = f.cfg(Duration::from_secs(10), Duration::from_secs(2));
        let h1 = m.new_run_handle();
        let t = spawn_with_handle(&m, cfg.clone(), "first", h1);
        assert!(wait_for(5, || f.touched("started")), "turn never started");
        let h2 = m.new_run_handle();
        let second = m.prompt_with_handle(&cfg, "second", opts(), &h2);
        assert!(matches!(second, Err(AcpAgentError::Busy)), "{second:?}");
        assert!(
            !m.cancel(&h2),
            "a busy caller holds no run and cannot cancel"
        );
        f.release("gate");
        let r = join_within(t, 5).unwrap();
        assert_eq!(r.text, "first-done");
        assert_eq!(
            f.recv("session/prompt").len(),
            1,
            "the busy call never reached the agent"
        );
    }

    #[test]
    fn stale_handle_cancel_is_noop() {
        let f = Fake::plain(
            "stale",
            vec![
                vec![chunk("one"), finish("end_turn")],
                vec![
                    touch("started"),
                    wait_file("gate"),
                    chunk("two"),
                    finish("end_turn"),
                ],
            ],
        );
        let m = Arc::new(AcpAgentManager::new());
        let cfg = f.cfg(Duration::from_secs(10), Duration::from_secs(2));
        let h1 = m.new_run_handle();
        let r1 = m.prompt_with_handle(&cfg, "one", opts(), &h1).unwrap();
        assert_eq!(r1.text, "one");
        assert!(!m.cancel(&h1), "cancel after completion is a no-op");
        let h2 = m.new_run_handle();
        let t = spawn_with_handle(&m, cfg.clone(), "two", h2);
        assert!(
            wait_for(5, || f.touched("started")),
            "second turn never started"
        );
        assert!(
            !m.cancel(&h1),
            "a stale handle must never cancel the newer turn"
        );
        f.release("gate");
        let r2 = join_within(t, 5).unwrap();
        assert_eq!(r2.text, "two");
        assert_eq!(f.starts(), 1);
    }

    #[test]
    fn cancel_ignored_by_agent_is_abandoned_after_grace_and_child_retired() {
        let f = Fake::new(
            "ignore",
            vec![
                vec![touch("started"), wait_cancel(), finish("end_turn")],
                vec![chunk("fresh"), finish("end_turn")],
            ],
            json!({"on_cancel": "ignore"}),
        );
        let m = Arc::new(AcpAgentManager::new());
        let cfg = f.cfg(Duration::from_secs(10), Duration::from_secs(1));
        let h = m.new_run_handle();
        let t = spawn_with_handle(&m, cfg.clone(), "first", h);
        assert!(wait_for(5, || f.touched("started")), "turn never started");
        assert!(m.cancel(&h));
        let cancelled_at = Instant::now();
        let out = join_within(t, 8);
        assert!(out.is_err(), "an abandoned turn must not report success");
        assert!(
            cancelled_at.elapsed() < Duration::from_secs(5),
            "abandon must follow the 1 s grace period"
        );
        assert!(
            !f.recv("session/cancel").is_empty(),
            "the cancel must reach the agent"
        );
        let r = m.prompt(&cfg, "again", opts()).unwrap();
        assert_eq!(r.text, "fresh");
        assert_eq!(
            f.starts(),
            2,
            "the ignoring child is retired, a fresh one starts"
        );
    }

    #[test]
    fn cancel_invalidates_pending_approval_late_allow_writes_nothing() {
        let f = Fake::plain(
            "invalidate",
            vec![vec![
                chunk("x"),
                ask(json!(9), "edit", None),
                await_resp(json!(9)),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        // Nobody answers before the cancel.
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        let h = m.new_run_handle();
        let t = spawn_with_handle(
            &m,
            f.cfg(Duration::from_secs(10), Duration::from_secs(2)),
            "hi",
            h,
        );
        assert!(wait_for(5, || p.views().len() == 1), "approval never shown");
        let token = p.views()[0].token.clone();
        assert!(m.cancel(&h));
        let out = join_within(t, 5);
        assert!(matches!(out, Err(AcpAgentError::Cancelled)), "{out:?}");
        let broker = m.permission_broker().unwrap();
        assert!(
            !broker.resolve(&token, UserChoice::Allow),
            "a late Allow must be refused after cancel"
        );
        assert!(
            f.response(&json!(9)).is_none(),
            "no response may be written for an invalidated request"
        );
    }

    #[test]
    fn permission_delivery_failure_ends_turn_and_closes_prompt() {
        let f = Fake::plain(
            "delivery",
            vec![vec![
                chunk("x"),
                ask(json!(7), "edit", None),
                await_resp(json!(7)),
                finish("end_turn"),
            ]],
        );
        let p = Arc::new(TestPrompter::default());
        let m = perm_manager(&p, PermissionMode::AskWrites, Duration::from_secs(60));
        m.force_permission_delivery_failure(true);
        let t = spawn_prompt(
            &m,
            f.cfg(Duration::from_secs(10), Duration::from_secs(2)),
            "hi",
            true,
        );
        assert!(wait_for(5, || p.views().len() == 1), "approval never shown");
        let token = p.views()[0].token.clone();
        let broker = m.permission_broker().unwrap();
        let approved_at = Instant::now();
        assert!(broker.resolve(&token, UserChoice::Allow));
        let out = join_within(t, 5);
        assert!(
            approved_at.elapsed() < Duration::from_secs(1),
            "delivery failure must end the turn promptly"
        );
        assert!(
            matches!(out, Err(AcpAgentError::PermissionDelivery)),
            "{out:?}"
        );
        assert_eq!(p.closed(), vec![token], "the prompt must be closed");
        assert!(
            f.response(&json!(7))
                .map_or(true, |e| e["result"]["outcome"]["outcome"] != "selected"),
            "no selected response may reach the agent"
        );
    }
}
