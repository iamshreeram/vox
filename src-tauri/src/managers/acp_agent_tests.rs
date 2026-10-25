//! Contract tests for the persistent ACP client (DESIGN.md rev 3.1, tests T1-T30).
//! Red until step S1 implements `acp_agent.rs`. Workers must NOT edit this file.
//!
//! The fake agent is a python3 script written to a per-test temp dir and driven by flags.
//! It is started as `<python> <script> <flags>` (no executable bit, so no ETXTBSY races).
//! Observations come from a JSONL log the fake appends to. No fixed sleeps for
//! correctness: readiness files and bounded polling; the only `sleep`s are documented
//! "settle" delays used to let a second caller arrive while a first one is blocked.

#[cfg(unix)]
mod unix_tests {
    use crate::managers::acp_agent::*;
    use serde_json::Value;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};

    const OK_TEXT: &str = "Two plus two is four.";

    const FAKE: &str = r##"
import sys, os, json, time, select, subprocess

A = sys.argv[1:]

def opt(name, default=None):
    if name in A:
        i = A.index(name)
        if i + 1 < len(A) and not A[i + 1].startswith("--"):
            return A[i + 1]
        return True
    return default

def val(name, default=None):
    v = opt(name, default)
    return None if v is True else v

def flag(n):
    return n in A

def touch(p):
    if p:
        open(p, "a").close()

LOG = val("--log")

def log(ev):
    if LOG:
        with open(LOG, "a") as f:
            f.write(json.dumps(ev) + "\n")

out = sys.stdout.buffer

def raw(b):
    out.write(b)
    out.flush()

def send(obj):
    raw((json.dumps(obj) + "\n").encode())

def summ(m):
    method = m.get("method")
    params = m.get("params") or {}
    if method == "session/prompt":
        text = (params.get("prompt") or [{}])[0].get("text", "")
        params = {"sessionId": params.get("sessionId"), "len": len(text), "head": text[:60]}
    return {"event": "recv", "method": method, "id": m.get("id"), "params": params,
            "result": m.get("result"), "error": m.get("error")}

class Rd:
    def __init__(self):
        self.buf = b""
        self.eof = False
    def get(self, timeout=None):
        end = None if timeout is None else time.time() + timeout
        while True:
            if b"\n" in self.buf:
                line, self.buf = self.buf.split(b"\n", 1)
                if not line.strip():
                    continue
                try:
                    return json.loads(line)
                except Exception:
                    continue
            if self.eof:
                return "EOF"
            t = None if end is None else max(0.0, end - time.time())
            r, _, _ = select.select([0], [], [], t)
            if not r:
                return None
            chunk_bytes = os.read(0, 65536)
            if not chunk_bytes:
                self.eof = True
                continue
            self.buf += chunk_bytes

rd = Rd()
sessions = {}
counter = [0]
log({"event": "start", "pid": os.getpid()})

if flag("--junk"):
    raw(b"\x1b]11;#1a1b26\x07\x1b]10;#c0caf5\x07")

if flag("--grandchild") or flag("--escaper"):
    child = subprocess.Popen(["sleep", "300"], start_new_session=flag("--escaper"))
    gp = val("--gpidfile")
    if gp:
        with open(gp, "w") as f:
            f.write(str(child.pid))

def noise():
    if flag("--noise"):
        raw(b"plain banner line, not json\n")

def err(msg_id, code, message):
    send({"jsonrpc": "2.0", "id": msg_id, "error": {"code": code, "message": message}})

def chunk(sid, text, kind="agent_message_chunk"):
    send({"jsonrpc": "2.0", "method": "session/update",
          "params": {"sessionId": sid, "update": {"sessionUpdate": kind,
                     "content": {"type": "text", "text": text}}}})

def wait_for_response(rid, limit=30):
    t0 = time.time()
    while time.time() - t0 < limit:
        m = rd.get(0.05)
        if m == "EOF":
            os._exit(0)
        if isinstance(m, dict):
            log(summ(m))
            if "method" not in m and m.get("id") == rid:
                return m
    return {}

def serve_until(pred, limit):
    t0 = time.time()
    while time.time() - t0 < limit:
        if pred():
            return None
        m = rd.get(0.02)
        if m == "EOF":
            os._exit(0)
        if isinstance(m, dict):
            log(summ(m))
            if m.get("method") == "session/cancel":
                return "cancel"
    return "timeout"

def do_initialize(m):
    if flag("--handshake-hang"):
        time.sleep(3600)
    d = val("--init-delay-file")
    if d:
        t0 = time.time()
        while not os.path.exists(d) and time.time() - t0 < 120:
            time.sleep(0.02)
    noise()
    if flag("--init-error"):
        err(m["id"], -32000, "x" * 1000 if flag("--long-error") else "init failed")
        return
    if flag("--malformed-init"):
        send({"jsonrpc": "2.0", "id": m["id"], "result": "nonsense"})
        return
    send({"jsonrpc": "2.0", "id": m["id"], "result": {
        "protocolVersion": int(val("--proto", "1")), "agentCapabilities": {},
        "agentInfo": {"name": val("--name", "fake-agent"), "version": "0"}, "authMethods": []}})

def do_new(m):
    if flag("--new-error"):
        err(m["id"], -32000, "session failed")
        return
    if flag("--new-auth-error"):
        err(m["id"], -32000, "authentication required")
        return
    if flag("--no-session-id"):
        send({"jsonrpc": "2.0", "id": m["id"], "result": {}})
        return
    counter[0] += 1
    sid = "sess-%d-%d" % (os.getpid(), counter[0])
    sessions[sid] = {}
    send({"jsonrpc": "2.0", "id": m["id"], "result": {"sessionId": sid, "configOptions": []}})
    if flag("--stuck-stdin"):
        time.sleep(3600)
    if flag("--close-stdout"):
        os.close(1)
        time.sleep(3600)

def finish_cancelled(m, sid):
    cm = val("--cancel-mode", "cancelled")
    if flag("--chunks-during-cancel"):
        chunk(sid, "partial-during-cancel")
    if cm == "cancelled":
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"stopReason": "cancelled"}})
    elif cm == "end_turn":
        chunk(sid, "late text")
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"stopReason": "end_turn"}})
    else:
        time.sleep(3600)

def do_prompt(m):
    sid = m["params"]["sessionId"]
    text = m["params"]["prompt"][0]["text"]
    touch(val("--prompt-started"))
    if flag("--crash"):
        sys.stderr.write("boom: agent crashed\n")
        sys.stderr.flush()
        os._exit(3)
    sf = val("--stderr-flood")
    if sf:
        sys.stderr.write("e" * int(sf) + "\n")
        sys.stderr.flush()
    hang_once = val("--hang-once")
    do_hang = flag("--hang") or (hang_once and not os.path.exists(hang_once))
    if hang_once and do_hang:
        touch(hang_once)
    if do_hang:
        r = serve_until(lambda: False, 3600)
        if r == "cancel":
            finish_cancelled(m, sid)
        return
    gate = val("--prompt-gate")
    if gate:
        used = gate + ".used"
        if not (flag("--gate-once") and os.path.exists(used)):
            touch(used)
            r = serve_until(lambda: os.path.exists(gate), 600)
            if r == "cancel":
                finish_cancelled(m, sid)
                return
    big = val("--big-line")
    if big:
        raw(b"x" * int(big))
        time.sleep(3600)
    if flag("--flood-chunks"):
        i = A.index("--flood-chunks")
        n, size = int(A[i + 1]), int(A[i + 2])
        for _ in range(n):
            chunk(sid, "a" * size)
    nf = val("--notif-flood")
    if nf:
        note = {"jsonrpc": "2.0", "method": "session/update",
                "params": {"sessionId": sid, "update": {"sessionUpdate": "plan", "entries": []}}}
        for _ in range(int(nf)):
            send(note)
    pf = val("--perm-flood")
    if pf:
        for i in range(int(pf)):
            send({"jsonrpc": "2.0", "id": "pf-%d" % i, "method": "session/request_permission",
                  "params": {"sessionId": sid, "toolCall": {"toolCallId": "t"}, "options": []}})
        time.sleep(3600)
    if flag("--permission"):
        rid = 3 if flag("--overlap-ids") else "perm-1"
        send({"jsonrpc": "2.0", "id": rid, "method": "session/request_permission",
              "params": {"sessionId": sid, "toolCall": {"toolCallId": "t1", "title": "write file"},
                         "options": [{"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                                     {"optionId": "reject", "name": "Reject", "kind": "reject_once"}]}})
        nat = val("--native")
        if nat:
            with open(nat, "w") as f:
                f.write("native action done")
        resp = wait_for_response(rid)
        log({"event": "permission_response", "result": resp.get("result"), "error": resp.get("error")})
    if flag("--unknown-request"):
        rid = 2 if flag("--overlap-ids") else 77
        send({"jsonrpc": "2.0", "id": rid, "method": "fs/read_text_file",
              "params": {"sessionId": sid, "path": "/etc/hosts"}})
        resp = wait_for_response(rid)
        log({"event": "fs_response", "error": resp.get("error")})
    if flag("--stray-response"):
        send({"jsonrpc": "2.0", "id": 99999, "result": {}})
    if flag("--wrong-session"):
        chunk("some-other-session", "WRONG")
    if flag("--thought"):
        chunk(sid, "thinking hard", "agent_thought_chunk")
        send({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": sid, "update": {"sessionUpdate": "tool_call", "toolCallId": "t9",
                                                      "title": "reading", "status": "pending"}}})
        send({"jsonrpc": "2.0", "method": "session/update",
              "params": {"sessionId": sid, "update": {"sessionUpdate": "mystery_kind"}}})
    if flag("--echo"):
        reply = "echo:" + text
    elif flag("--memory"):
        s = sessions.setdefault(sid, {})
        if text.startswith("remember "):
            s["secret"] = text[9:]
            reply = "ok"
        elif text.startswith("recall"):
            reply = "secret=" + s.get("secret", "none")
        else:
            reply = "ok"
    else:
        reply = val("--reply", "Two plus two is four.")
        if flag("--reply-repeat"):
            i = A.index("--reply-repeat")
            reply = A[i + 1] * int(A[i + 2])
    if not flag("--empty"):
        n = max(1, int(val("--chunks", "3")))
        size = max(1, (len(reply) + n - 1) // n)
        for i in range(0, len(reply), size):
            chunk(sid, reply[i:i + size])
    stop = val("--stop", "end_turn")
    result = {} if stop == "none" else {"stopReason": stop}
    send({"jsonrpc": "2.0", "id": m["id"], "result": result})
    if flag("--idle-chunk"):
        chunk(sid, "IDLE")

while True:
    m = rd.get()
    if m == "EOF":
        break
    if not isinstance(m, dict):
        continue
    log(summ(m))
    method = m.get("method")
    if method == "initialize":
        do_initialize(m)
    elif method == "session/new":
        do_new(m)
    elif method == "session/prompt":
        do_prompt(m)
    elif method == "session/close":
        send({"jsonrpc": "2.0", "id": m.get("id"), "result": {}})
    elif method and "id" in m:
        err(m["id"], -32601, "method not found")
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

    fn pid_alive(pid: i32) -> bool {
        // SAFETY: signal 0 only probes existence.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn wait_until(secs: u64, f: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        f()
    }

    fn opts(reuse_session: bool) -> PromptOptions {
        PromptOptions {
            reuse_session,
            expected_epoch: None,
        }
    }

    struct Fake {
        dir: PathBuf,
    }

    impl Fake {
        fn new(tag: &str) -> Fake {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let dir =
                std::env::temp_dir().join(format!("vox-acp-{tag}-{}-{n}", std::process::id()));
            fs::create_dir_all(dir.join("cwd")).unwrap();
            fs::write(dir.join("fake_agent.py"), FAKE).unwrap();
            Fake { dir }
        }

        fn p(&self, name: &str) -> String {
            self.dir.join(name).to_string_lossy().into_owned()
        }

        fn cfg(&self, flags: &[&str]) -> AcpAgentConfig {
            let mut args = vec![
                self.p("fake_agent.py"),
                "--log".to_string(),
                self.p("log.jsonl"),
                "--gpidfile".to_string(),
                self.p("gpid"),
            ];
            args.extend(flags.iter().map(|s| s.to_string()));
            AcpAgentConfig {
                command: python().clone(),
                args,
                cwd: self.dir.join("cwd"),
                turn_timeout: Duration::from_secs(30),
                handshake_timeout: Duration::from_secs(30),
                cancel_grace: Duration::from_secs(2),
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

        fn named(&self, event: &str) -> Vec<Value> {
            self.events()
                .into_iter()
                .filter(|e| e["event"] == event)
                .collect()
        }

        fn pids(&self) -> Vec<i32> {
            self.named("start")
                .iter()
                .filter_map(|e| e["pid"].as_i64().map(|p| p as i32))
                .collect()
        }

        fn starts(&self) -> usize {
            self.named("start").len()
        }

        fn gpid(&self) -> Option<i32> {
            fs::read_to_string(self.dir.join("gpid"))
                .ok()
                .and_then(|s| s.trim().parse().ok())
        }

        fn session_ids(&self) -> Vec<String> {
            self.recv("session/prompt")
                .iter()
                .map(|e| e["params"]["sessionId"].as_str().unwrap_or("").to_string())
                .collect()
        }

        fn touched(&self, name: &str) -> bool {
            self.dir.join(name).exists()
        }

        fn release(&self, name: &str) {
            fs::write(self.dir.join(name), b"go").unwrap();
        }

        fn is_ours(&self, pid: i32) -> bool {
            let marker = self.dir.to_string_lossy().into_owned();
            std::process::Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "command="])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains(&marker))
                .unwrap_or(false)
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let mut all = self.pids();
            if let Some(g) = self.gpid() {
                all.push(g);
            }
            for pid in all {
                // Only signal processes that are still ours (guards against pid reuse).
                if pid_alive(pid) && (self.is_ours(pid) || self.gpid() == Some(pid)) {
                    // SAFETY: best-effort cleanup of this test's own children.
                    unsafe {
                        libc::killpg(pid, libc::SIGKILL);
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    // ---------------------------------------------------------------- prerequisites

    #[test]
    fn prerequisite_python3_is_available() {
        assert!(
            python().is_file(),
            "PREREQUISITE: python3 interpreter not found; install python3 to run the ACP tests"
        );
    }

    // ---------------------------------------------------------------- T1-T4

    #[test]
    fn t01_two_turns_reuse_one_child_and_one_session() {
        let f = Fake::new("t01");
        let m = AcpAgentManager::new();
        let cfg = f.cfg(&[]);
        let r1 = m.prompt(&cfg, "first", opts(true)).unwrap();
        assert_eq!(r1.text, OK_TEXT);
        assert_eq!(r1.agent_name.as_deref(), Some("fake-agent"));
        let r2 = m.prompt(&cfg, "second", opts(true)).unwrap();
        assert_eq!(r2.text, OK_TEXT);
        assert_eq!(f.starts(), 1, "one child for both turns");
        assert_eq!(f.recv("session/new").len(), 1, "one session for both turns");
        let sids = f.session_ids();
        assert_eq!(sids.len(), 2);
        assert_eq!(sids[0], sids[1]);
        assert!(m.is_running());
        assert_eq!(
            m.inner_strong_count(),
            1,
            "reaper and helpers must hold only weak refs"
        );
    }

    #[test]
    fn t02_context_off_uses_a_fresh_session_per_turn_on_the_same_child() {
        let f = Fake::new("t02");
        let m = AcpAgentManager::new();
        let cfg = f.cfg(&[]);
        m.prompt(&cfg, "one", opts(false)).unwrap();
        m.prompt(&cfg, "two", opts(false)).unwrap();
        assert_eq!(f.starts(), 1, "the child stays warm");
        let sids = f.session_ids();
        assert_eq!(sids.len(), 2);
        assert_ne!(sids[0], sids[1], "fresh session each turn");
        assert!(
            wait_until(5, || !f.recv("session/close").is_empty()),
            "a best-effort session/close must be sent"
        );
    }

    #[test]
    fn t03_chunks_join_in_order_and_non_message_updates_are_ignored() {
        let f = Fake::new("t03");
        let m = AcpAgentManager::new();
        let r = m
            .prompt(&f.cfg(&["--thought", "--chunks", "5"]), "hi", opts(true))
            .unwrap();
        assert_eq!(r.text, OK_TEXT);
    }

    #[test]
    fn t04_escape_junk_before_the_first_json_and_non_json_lines_are_tolerated() {
        let f = Fake::new("t04");
        let m = AcpAgentManager::new();
        let r = m
            .prompt(&f.cfg(&["--junk", "--noise"]), "hi", opts(true))
            .unwrap();
        assert_eq!(r.text, OK_TEXT);
    }

    // ---------------------------------------------------------------- T5-T8

    #[test]
    fn t05_permission_requests_are_always_denied_but_native_agent_action_still_happens() {
        let f = Fake::new("t05");
        let native = f.p("native.txt");
        let m = AcpAgentManager::new();
        let r = m
            .prompt(
                &f.cfg(&["--permission", "--native", native.as_str()]),
                "hi",
                opts(true),
            )
            .unwrap();
        assert_eq!(r.text, OK_TEXT);
        let responses = f.named("permission_response");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["result"]["outcome"]["outcome"], "cancelled");
        assert!(
            f.touched("native.txt"),
            "denying a client permission does not stop the agent's own tools"
        );
    }

    #[test]
    fn t06_unknown_agent_requests_get_method_not_found_and_overlapping_ids_are_harmless() {
        let f = Fake::new("t06");
        let m = AcpAgentManager::new();
        let r = m
            .prompt(
                &f.cfg(&["--unknown-request", "--permission", "--overlap-ids"]),
                "hi",
                opts(true),
            )
            .unwrap();
        assert_eq!(r.text, OK_TEXT);
        let fs_resp = f.named("fs_response");
        assert_eq!(fs_resp.len(), 1);
        assert_eq!(fs_resp[0]["error"]["code"], -32601);
        let perm = f.named("permission_response");
        assert_eq!(perm[0]["result"]["outcome"]["outcome"], "cancelled");
    }

    #[test]
    fn t07_blank_prompt_is_rejected_before_any_spawn() {
        let f = Fake::new("t07");
        let m = AcpAgentManager::new();
        let err = m.prompt(&f.cfg(&[]), "  \n\t ", opts(true)).unwrap_err();
        assert!(matches!(err, AcpAgentError::Protocol(_)), "{err:?}");
        assert_eq!(f.starts(), 0, "no child may be started for a blank prompt");
        assert!(!m.is_running());
    }

    #[test]
    fn t08_missing_command_and_non_executable_command_are_distinct_errors() {
        let f = Fake::new("t08");
        let m = AcpAgentManager::new();
        let mut cfg = f.cfg(&[]);
        cfg.command = f.dir.join("does-not-exist");
        let err = m.prompt(&cfg, "hi", opts(true)).unwrap_err();
        assert!(matches!(err, AcpAgentError::CommandNotFound), "{err:?}");
        let plain = f.dir.join("plain.txt");
        fs::write(&plain, "not a program").unwrap();
        cfg.command = plain;
        let err = m.prompt(&cfg, "hi", opts(true)).unwrap_err();
        assert!(matches!(err, AcpAgentError::SpawnFailed(_)), "{err:?}");
        assert!(!m.is_running());
    }

    // ---------------------------------------------------------------- T9, T9b

    #[test]
    fn t09_a_handshake_that_never_answers_fails_within_the_handshake_timeout() {
        let f = Fake::new("t09");
        let m = AcpAgentManager::new();
        let mut cfg = f.cfg(&["--handshake-hang"]);
        cfg.handshake_timeout = Duration::from_secs(1);
        let started = Instant::now();
        let err = m.prompt(&cfg, "hi", opts(true)).unwrap_err();
        assert!(matches!(err, AcpAgentError::HandshakeFailed(_)), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(15));
        let pid = f.pids()[0];
        assert!(wait_until(5, || !pid_alive(pid)), "child must be dead");
        assert!(!m.is_running());
    }

    #[test]
    fn t09b_every_handshake_failure_is_immediate_capped_clean_and_recoverable() {
        let variants: Vec<(&str, Vec<&str>)> = vec![
            ("init_error", vec!["--init-error", "--long-error"]),
            ("proto2", vec!["--proto", "2"]),
            ("malformed", vec!["--malformed-init"]),
            ("new_error", vec!["--new-error"]),
            ("new_auth", vec!["--new-auth-error"]),
            ("no_session_id", vec!["--no-session-id"]),
        ];
        let m = AcpAgentManager::new();
        for (name, flags) in variants {
            let f = Fake::new(name);
            let started = Instant::now();
            let err = m.prompt(&f.cfg(&flags), "hi", opts(true)).unwrap_err();
            match err {
                AcpAgentError::HandshakeFailed(msg) => assert!(
                    msg.chars().count() <= MAX_ERR_DETAIL_CHARS + 100,
                    "{name}: agent text not capped ({} chars)",
                    msg.chars().count()
                ),
                other => panic!("{name}: expected HandshakeFailed, got {other:?}"),
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "{name}: not immediate"
            );
            assert!(
                f.recv("session/prompt").is_empty(),
                "{name}: no prompt may be sent"
            );
            let pid = f.pids()[0];
            assert!(
                wait_until(5, || !pid_alive(pid)),
                "{name}: child left behind"
            );
            assert!(!m.is_running(), "{name}");
        }
        let good = Fake::new("t09b_good");
        let r = m.prompt(&good.cfg(&[]), "hi", opts(true)).unwrap();
        assert_eq!(r.text, OK_TEXT, "the next call starts clean");
    }

    // ---------------------------------------------------------------- T10

    #[test]
    fn t10_a_turn_timeout_cancels_discards_text_kills_the_child_and_respawns_next_time() {
        let variants: Vec<(&str, Vec<&str>)> = vec![
            ("cancelled", vec!["--cancel-mode", "cancelled"]),
            ("end_turn", vec!["--cancel-mode", "end_turn"]),
            (
                "chunks",
                vec!["--cancel-mode", "cancelled", "--chunks-during-cancel"],
            ),
            ("ignore", vec!["--cancel-mode", "ignore"]),
        ];
        for (name, extra) in variants {
            let f = Fake::new(&format!("t10_{name}"));
            let marker = f.p("hung.marker");
            let mut flags: Vec<&str> = vec!["--hang-once", marker.as_str()];
            flags.extend(extra);
            let mut cfg = f.cfg(&flags);
            cfg.turn_timeout = Duration::from_secs(1);
            cfg.cancel_grace = Duration::from_secs(1);
            let m = AcpAgentManager::new();
            let started = Instant::now();
            let err = m.prompt(&cfg, "hang", opts(true)).unwrap_err();
            assert!(matches!(err, AcpAgentError::Timeout), "{name}: {err:?}");
            assert!(started.elapsed() < Duration::from_secs(20), "{name}");
            assert!(
                wait_until(5, || !f.recv("session/cancel").is_empty()),
                "{name}: session/cancel must be sent"
            );
            let pid1 = f.pids()[0];
            assert!(
                wait_until(5, || !pid_alive(pid1)),
                "{name}: child must always be killed"
            );
            assert!(!m.is_running(), "{name}");
            let r = m.prompt(&cfg, "again", opts(true)).unwrap();
            assert_eq!(
                r.text, OK_TEXT,
                "{name}: no text from the timed-out turn may leak"
            );
            assert_eq!(f.starts(), 2, "{name}");
            let pids = f.pids();
            assert_ne!(pids[0], pids[1], "{name}: fresh child expected");
            let sids = f.session_ids();
            assert_ne!(sids[0], sids[1], "{name}: fresh session expected");
        }
    }

    // ---------------------------------------------------------------- T11-T14b

    #[test]
    fn t11_a_crash_mid_turn_reports_exit_code_and_a_capped_stderr_tail() {
        let f = Fake::new("t11");
        let m = AcpAgentManager::new();
        let err = m
            .prompt(&f.cfg(&["--crash"]), "hi", opts(true))
            .unwrap_err();
        match err {
            AcpAgentError::AgentExited { code, stderr } => {
                assert_eq!(code, Some(3));
                assert!(stderr.contains("boom"), "stderr tail missing: {stderr:?}");
                assert!(stderr.chars().count() <= STDERR_TAIL_CHARS);
            }
            other => panic!("expected AgentExited, got {other:?}"),
        }
        assert!(!m.is_running());
    }

    #[test]
    fn t12_stop_reasons_and_empty_replies_are_mapped_deterministically() {
        type Check = fn(Result<AcpReply, AcpAgentError>) -> bool;
        let cases: Vec<(&str, Vec<&str>, Check)> = vec![
            ("refusal", vec!["--stop", "refusal"], |r| {
                matches!(r, Err(AcpAgentError::Refused))
            }),
            ("max_tokens", vec!["--stop", "max_tokens"], |r| {
                r.map(|x| x.text == OK_TEXT).unwrap_or(false)
            }),
            (
                "max_turn_requests_with_text",
                vec!["--stop", "max_turn_requests"],
                |r| r.map(|x| x.text == OK_TEXT).unwrap_or(false),
            ),
            (
                "max_turn_requests_empty",
                vec!["--stop", "max_turn_requests", "--empty"],
                |r| matches!(r, Err(AcpAgentError::Protocol(_))),
            ),
            ("unknown_stop", vec!["--stop", "weird"], |r| {
                matches!(r, Err(AcpAgentError::Protocol(_)))
            }),
            ("missing_stop", vec!["--stop", "none"], |r| {
                matches!(r, Err(AcpAgentError::Protocol(_)))
            }),
            ("empty_end_turn", vec!["--empty"], |r| {
                matches!(r, Err(AcpAgentError::EmptyReply))
            }),
        ];
        for (name, flags, check) in cases {
            let f = Fake::new(&format!("t12_{name}"));
            let m = AcpAgentManager::new();
            let result = m.prompt(&f.cfg(&flags), "hi", opts(true));
            assert!(check(result), "case {name} failed");
        }
    }

    #[test]
    fn t13_long_replies_are_cut_at_a_char_boundary() {
        let f = Fake::new("t13");
        let m = AcpAgentManager::new();
        let r = m
            .prompt(
                &f.cfg(&["--reply-repeat", "\u{e9}", "20000"]),
                "hi",
                opts(true),
            )
            .unwrap();
        assert_eq!(r.text.chars().count(), MAX_REPLY_CHARS);
        assert!(r.text.chars().all(|c| c == '\u{e9}'));
    }

    #[test]
    fn t14_an_oversized_unterminated_line_is_a_protocol_error_and_kills_the_child() {
        let f = Fake::new("t14");
        let m = AcpAgentManager::new();
        let mut cfg = f.cfg(&["--big-line", "5000000"]);
        cfg.turn_timeout = Duration::from_secs(20);
        let started = Instant::now();
        let err = m.prompt(&cfg, "hi", opts(true)).unwrap_err();
        assert!(matches!(err, AcpAgentError::Protocol(_)), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(18));
        let pid = f.pids()[0];
        assert!(wait_until(5, || !pid_alive(pid)));
    }

    #[test]
    fn t14b_many_small_chunks_beyond_the_aggregate_cap_fail_and_notification_floods_complete() {
        let f = Fake::new("t14b_chunks");
        let m = AcpAgentManager::new();
        let mut cfg = f.cfg(&["--flood-chunks", "3000", "200"]);
        cfg.turn_timeout = Duration::from_secs(25);
        let err = m.prompt(&cfg, "hi", opts(true)).unwrap_err();
        assert!(matches!(err, AcpAgentError::Protocol(_)), "{err:?}");
        let pid = f.pids()[0];
        assert!(wait_until(5, || !pid_alive(pid)));

        let g = Fake::new("t14b_flood");
        let m2 = AcpAgentManager::new();
        let mut cfg2 = g.cfg(&["--notif-flood", "100000"]);
        cfg2.turn_timeout = Duration::from_secs(40);
        let started = Instant::now();
        let r = m2.prompt(&cfg2, "hi", opts(true)).unwrap();
        assert_eq!(r.text, OK_TEXT);
        assert!(started.elapsed() < Duration::from_secs(40));
    }

    // ---------------------------------------------------------------- T15, T16

    #[test]
    fn t15a_a_second_prompt_during_a_turn_is_busy_and_the_first_still_completes() {
        let f = Fake::new("t15a");
        let gate = f.p("gate");
        let started = f.p("started");
        let cfg = f.cfg(&[
            "--prompt-gate",
            gate.as_str(),
            "--prompt-started",
            started.as_str(),
        ]);
        let m = Arc::new(AcpAgentManager::new());
        let (m2, cfg2) = (m.clone(), cfg.clone());
        let h = thread::spawn(move || m2.prompt(&cfg2, "slow", opts(true)));
        assert!(
            wait_until(30, || f.touched("started")),
            "turn never started"
        );
        let t = Instant::now();
        let err = m.prompt(&cfg, "second", opts(true)).unwrap_err();
        assert!(matches!(err, AcpAgentError::Busy), "{err:?}");
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "Busy must be immediate"
        );
        f.release("gate");
        let r = h.join().unwrap().unwrap();
        assert_eq!(r.text, OK_TEXT);
    }

    #[test]
    fn t15b_a_prompt_during_warm_up_joins_it_instead_of_failing_busy() {
        let f = Fake::new("t15b");
        let delay = f.p("delay");
        let cfg = f.cfg(&["--init-delay-file", delay.as_str()]);
        let m = Arc::new(AcpAgentManager::new());
        let (ma, ca) = (m.clone(), cfg.clone());
        let a = thread::spawn(move || ma.warm_up(&ca));
        assert!(
            wait_until(30, || f.recv("initialize").len() == 1),
            "warm-up never reached initialize"
        );
        let (mb, cb) = (m.clone(), cfg.clone());
        let b = thread::spawn(move || mb.prompt(&cb, "hello", opts(true)));
        // settle delay: give B time to arrive while warm-up is still blocked
        thread::sleep(Duration::from_millis(300));
        f.release("delay");
        let warm = a.join().unwrap().unwrap();
        assert_eq!(warm.as_deref(), Some("fake-agent"));
        let reply = b.join().unwrap().unwrap();
        assert_eq!(reply.text, OK_TEXT);
        assert_eq!(f.starts(), 1, "the prompt must reuse the warmed child");
    }

    #[test]
    fn t15c_two_warm_ups_with_the_same_config_join_each_other() {
        let f = Fake::new("t15c");
        let delay = f.p("delay");
        let cfg = f.cfg(&["--init-delay-file", delay.as_str()]);
        let m = Arc::new(AcpAgentManager::new());
        let (ma, ca) = (m.clone(), cfg.clone());
        let a = thread::spawn(move || ma.warm_up(&ca));
        assert!(wait_until(30, || f.recv("initialize").len() == 1));
        let (mb, cb) = (m.clone(), cfg.clone());
        let b = thread::spawn(move || mb.warm_up(&cb));
        thread::sleep(Duration::from_millis(300)); // settle delay
        f.release("delay");
        assert_eq!(a.join().unwrap().unwrap().as_deref(), Some("fake-agent"));
        assert_eq!(b.join().unwrap().unwrap().as_deref(), Some("fake-agent"));
        assert_eq!(f.starts(), 1);
    }

    #[test]
    fn t16_a_config_change_replaces_the_child_when_idle_and_is_busy_during_a_turn() {
        let f = Fake::new("t16");
        let m = Arc::new(AcpAgentManager::new());
        let cfg1 = f.cfg(&[]);
        let cfg2 = f.cfg(&["--tag", "b"]);
        m.prompt(&cfg1, "one", opts(true)).unwrap();
        let pid1 = f.pids()[0];
        m.prompt(&cfg2, "two", opts(true)).unwrap();
        assert_eq!(f.starts(), 2);
        assert!(wait_until(5, || !pid_alive(pid1)), "old child must be gone");

        let gate = f.p("gate");
        let started = f.p("started");
        let busy_cfg = f.cfg(&[
            "--prompt-gate",
            gate.as_str(),
            "--prompt-started",
            started.as_str(),
        ]);
        let (m2, c2) = (m.clone(), busy_cfg.clone());
        let h = thread::spawn(move || m2.prompt(&c2, "slow", opts(true)));
        assert!(wait_until(30, || f.touched("started")));
        let err = m.prompt(&cfg2, "other config", opts(true)).unwrap_err();
        assert!(matches!(err, AcpAgentError::Busy), "{err:?}");
        f.release("gate");
        assert_eq!(h.join().unwrap().unwrap().text, OK_TEXT);
    }

    // ---------------------------------------------------------------- T17, T18

    #[test]
    fn t17_shutdown_kills_the_whole_group_and_is_idempotent_and_recoverable() {
        let f = Fake::new("t17");
        let m = AcpAgentManager::new();
        let cfg = f.cfg(&["--grandchild"]);
        m.prompt(&cfg, "hi", opts(true)).unwrap();
        let pid = f.pids()[0];
        let grandchild = f.gpid().expect("grandchild pid");
        assert!(pid_alive(grandchild));
        m.shutdown();
        assert!(wait_until(5, || !pid_alive(pid) && !pid_alive(grandchild)));
        assert!(!m.is_running());
        m.shutdown(); // idempotent
        let r = m.prompt(&cfg, "again", opts(true)).unwrap();
        assert_eq!(r.text, OK_TEXT);
        assert_eq!(f.starts(), 2);
    }

    #[test]
    fn t17b_dropping_the_manager_kills_the_child() {
        let f = Fake::new("t17b");
        let pid;
        {
            let m = AcpAgentManager::new();
            m.prompt(&f.cfg(&[]), "hi", opts(true)).unwrap();
            pid = f.pids()[0];
            assert!(pid_alive(pid));
        }
        assert!(
            wait_until(5, || !pid_alive(pid)),
            "Drop must kill the child"
        );
    }

    #[test]
    fn t17c_close_is_irreversible_and_nothing_spawns_afterwards() {
        let f = Fake::new("t17c");
        let m = AcpAgentManager::new();
        let cfg = f.cfg(&[]);
        m.prompt(&cfg, "hi", opts(true)).unwrap();
        let pid = f.pids()[0];
        m.close();
        assert!(wait_until(5, || !pid_alive(pid)));
        assert!(matches!(
            m.prompt(&cfg, "again", opts(true)).unwrap_err(),
            AcpAgentError::Closed
        ));
        assert!(matches!(
            m.warm_up(&cfg).unwrap_err(),
            AcpAgentError::Closed
        ));
        assert_eq!(f.starts(), 1, "no child may start after close");
    }

    #[test]
    fn t18a_the_reaper_shuts_down_an_idle_child_and_the_next_prompt_respawns() {
        let f = Fake::new("t18a");
        let m = AcpAgentManager::with_idle_shutdown(Duration::from_millis(300));
        let cfg = f.cfg(&[]);
        m.prompt(&cfg, "hi", opts(true)).unwrap();
        let pid = f.pids()[0];
        assert!(
            wait_until(15, || !m.is_running()),
            "idle child never reaped"
        );
        assert!(wait_until(5, || !pid_alive(pid)));
        assert_eq!(m.inner_strong_count(), 1);
        let r = m.prompt(&cfg, "again", opts(true)).unwrap();
        assert_eq!(r.text, OK_TEXT);
        assert_eq!(f.starts(), 2);
    }

    #[test]
    fn t18b_the_reaper_never_kills_a_child_that_is_in_a_turn() {
        let f = Fake::new("t18b");
        let gate = f.p("gate");
        let started = f.p("started");
        let cfg = f.cfg(&[
            "--prompt-gate",
            gate.as_str(),
            "--prompt-started",
            started.as_str(),
        ]);
        let m = Arc::new(AcpAgentManager::with_idle_shutdown(Duration::from_millis(
            300,
        )));
        let (m2, c2) = (m.clone(), cfg.clone());
        let h = thread::spawn(move || m2.prompt(&c2, "slow", opts(true)));
        assert!(wait_until(30, || f.touched("started")));
        // settle delay: several idle periods pass while the turn is in flight
        thread::sleep(Duration::from_millis(1200));
        assert!(m.is_running(), "an active turn must not be reaped");
        f.release("gate");
        assert_eq!(h.join().unwrap().unwrap().text, OK_TEXT);
        assert_eq!(f.starts(), 1);
    }

    // ---------------------------------------------------------------- T19, T20

    #[test]
    fn t19_wrong_session_idle_and_stray_messages_never_reach_the_reply() {
        let f = Fake::new("t19");
        let m = AcpAgentManager::new();
        let r = m
            .prompt(
                &f.cfg(&["--wrong-session", "--idle-chunk", "--stray-response"]),
                "hi",
                opts(true),
            )
            .unwrap();
        assert_eq!(r.text, OK_TEXT);
    }

    #[test]
    fn t20_sanitize_reply_strips_terminal_sequences_and_controls_and_caps_length() {
        assert_eq!(sanitize_reply("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(sanitize_reply("\u{1b}]11;#1a1b26\u{7}text"), "text");
        assert_eq!(sanitize_reply("\u{1b}]0;title\u{1b}\\after"), "after");
        assert_eq!(sanitize_reply("  \u{e9}\n\tok \u{7} "), "\u{e9}\n\tok");
        assert_eq!(sanitize_reply("a\u{0}b\u{1f}c"), "abc");
        assert_eq!(sanitize_reply("emoji \u{1F600} ok"), "emoji \u{1F600} ok");
        let long = "\u{e9}".repeat(MAX_REPLY_CHARS + 50);
        assert_eq!(sanitize_reply(&long).chars().count(), MAX_REPLY_CHARS);
    }

    // ---------------------------------------------------------------- T21-T23

    #[test]
    fn t21_a_one_megabyte_stderr_flood_does_not_deadlock() {
        let f = Fake::new("t21");
        let m = AcpAgentManager::new();
        let r = m
            .prompt(&f.cfg(&["--stderr-flood", "1000000"]), "hi", opts(true))
            .unwrap();
        assert_eq!(r.text, OK_TEXT);
    }

    #[test]
    fn t22_prompts_with_quotes_newlines_and_unicode_arrive_byte_exact() {
        let f = Fake::new("t22");
        let m = AcpAgentManager::new();
        let text = "He said \"hi\"\nline2 \\ \u{2713} \u{e9} \u{1F600}";
        let r = m.prompt(&f.cfg(&["--echo"]), text, opts(true)).unwrap();
        assert_eq!(r.text, format!("echo:{text}"));
    }

    #[test]
    fn t23_a_child_that_stops_reading_stdin_cannot_hang_the_turn() {
        let f = Fake::new("t23");
        let m = AcpAgentManager::new();
        let mut cfg = f.cfg(&["--stuck-stdin"]);
        cfg.turn_timeout = Duration::from_secs(2);
        let big = "x".repeat(4 * 1024 * 1024);
        let started = Instant::now();
        let err = m.prompt(&cfg, &big, opts(true)).unwrap_err();
        assert!(matches!(err, AcpAgentError::Timeout), "{err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(12),
            "write phase must be covered by the deadline"
        );
        let pid = f.pids()[0];
        assert!(wait_until(5, || !pid_alive(pid)), "child must be reaped");
        let t = Instant::now();
        m.shutdown();
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "shutdown must stay bounded"
        );
    }

    // ---------------------------------------------------------------- T24-T26

    #[test]
    fn t24_context_controls_isolate_conversations() {
        let f = Fake::new("t24");
        let m = AcpAgentManager::new();
        let cfg = f.cfg(&["--memory"]);
        assert_eq!(
            m.prompt(&cfg, "remember apple", opts(true)).unwrap().text,
            "ok"
        );
        assert_eq!(
            m.prompt(&cfg, "recall", opts(true)).unwrap().text,
            "secret=apple",
            "reused session keeps context"
        );
        m.reset_session();
        assert_eq!(
            m.prompt(&cfg, "recall", opts(true)).unwrap().text,
            "secret=none",
            "reset_session must drop the conversation"
        );
        assert_eq!(
            m.prompt(&cfg, "remember pear", opts(false)).unwrap().text,
            "ok"
        );
        assert_eq!(
            m.prompt(&cfg, "recall", opts(false)).unwrap().text,
            "secret=none",
            "fresh session per turn must not remember"
        );
    }

    #[test]
    fn t25_shutdown_during_a_blocked_turn_wakes_it_with_an_error_never_a_reply() {
        let f = Fake::new("t25");
        let gate = f.p("gate");
        let started = f.p("started");
        let cfg = f.cfg(&[
            "--prompt-gate",
            gate.as_str(),
            "--prompt-started",
            started.as_str(),
        ]);
        let m = Arc::new(AcpAgentManager::new());
        let (m2, c2) = (m.clone(), cfg.clone());
        let h = thread::spawn(move || m2.prompt(&c2, "slow", opts(true)));
        assert!(wait_until(30, || f.touched("started")));
        let t = Instant::now();
        m.shutdown();
        let result = h.join().unwrap();
        assert!(
            result.is_err(),
            "a shut-down turn must never return a reply"
        );
        assert!(t.elapsed() < Duration::from_secs(15));
        let pid = f.pids()[0];
        assert!(wait_until(5, || !pid_alive(pid)));
    }

    #[test]
    fn t26_reset_during_a_turn_is_stale_and_the_manager_is_immediately_usable_again() {
        let f = Fake::new("t26");
        let gate = f.p("gate");
        let started = f.p("started");
        let cfg = f.cfg(&[
            "--memory",
            "--prompt-gate",
            gate.as_str(),
            "--prompt-started",
            started.as_str(),
            "--gate-once",
        ]);
        let m = Arc::new(AcpAgentManager::new());
        let (m2, c2) = (m.clone(), cfg.clone());
        let h = thread::spawn(move || m2.prompt(&c2, "remember secret1", opts(true)));
        assert!(wait_until(30, || f.touched("started")));
        m.reset_session();
        f.release("gate");
        let stale = h.join().unwrap().unwrap_err();
        assert!(matches!(stale, AcpAgentError::Stale), "{stale:?}");
        // not stuck Busy, and the pre-reset conversation is gone
        let r = m.prompt(&cfg, "recall", opts(true)).unwrap();
        assert_eq!(r.text, "secret=none");
    }

    // ---------------------------------------------------------------- T27

    #[test]
    fn t27a_shutdown_during_an_unpublished_spawn_never_resurrects_or_clobbers() {
        let f = Fake::new("t27a");
        let delay = f.p("delay");
        let cfg1 = f.cfg(&["--init-delay-file", delay.as_str()]);
        let cfg2 = f.cfg(&["--tag", "b"]);
        let m = Arc::new(AcpAgentManager::new());
        let (ma, ca) = (m.clone(), cfg1.clone());
        let a = thread::spawn(move || ma.warm_up(&ca));
        assert!(wait_until(30, || f.recv("initialize").len() == 1));
        let pid1 = f.pids()[0];
        m.shutdown();
        let r = m.prompt(&cfg2, "hi", opts(true)).unwrap();
        assert_eq!(r.text, OK_TEXT);
        let pid2 = f.pids()[1];
        f.release("delay");
        let stale = a.join().unwrap();
        assert!(
            stale.is_err(),
            "the stale warm_up must not succeed: {stale:?}"
        );
        assert!(
            wait_until(5, || !pid_alive(pid1)),
            "old child must be killed"
        );
        assert!(
            pid_alive(pid2),
            "the newer reservation's child must be untouched"
        );
        assert!(m.is_running());
        let again = m.prompt(&cfg2, "again", opts(true)).unwrap();
        assert_eq!(again.text, OK_TEXT);
        assert_eq!(f.starts(), 2, "no third spawn");
    }

    #[test]
    fn t27b_close_during_an_unpublished_spawn_leaves_no_child() {
        let f = Fake::new("t27b");
        let delay = f.p("delay");
        let cfg = f.cfg(&["--init-delay-file", delay.as_str()]);
        let m = Arc::new(AcpAgentManager::new());
        let (ma, ca) = (m.clone(), cfg.clone());
        let a = thread::spawn(move || ma.warm_up(&ca));
        assert!(wait_until(30, || f.recv("initialize").len() == 1));
        let pid1 = f.pids()[0];
        m.close();
        f.release("delay");
        assert!(a.join().unwrap().is_err());
        assert!(wait_until(5, || !pid_alive(pid1)));
        assert!(matches!(
            m.prompt(&cfg, "hi", opts(true)).unwrap_err(),
            AcpAgentError::Closed
        ));
        assert_eq!(f.starts(), 1);
        assert!(!m.is_running());
    }

    #[test]
    fn t27c_a_warm_up_with_a_different_config_during_warming_is_busy() {
        let f = Fake::new("t27c");
        let delay = f.p("delay");
        let cfg1 = f.cfg(&["--init-delay-file", delay.as_str()]);
        let cfg2 = f.cfg(&["--tag", "other"]);
        let m = Arc::new(AcpAgentManager::new());
        let (ma, ca) = (m.clone(), cfg1.clone());
        let a = thread::spawn(move || ma.warm_up(&ca));
        assert!(wait_until(30, || f.recv("initialize").len() == 1));
        let err = m.warm_up(&cfg2).unwrap_err();
        assert!(matches!(err, AcpAgentError::Busy), "{err:?}");
        f.release("delay");
        assert_eq!(a.join().unwrap().unwrap().as_deref(), Some("fake-agent"));
    }

    // ---------------------------------------------------------------- T28-T30

    #[test]
    fn t28_stdout_closed_while_the_child_lives_fails_waiters_fast_and_frees_helpers() {
        let f = Fake::new("t28");
        let m = AcpAgentManager::new();
        let started = Instant::now();
        let err = m
            .prompt(&f.cfg(&["--close-stdout"]), "hi", opts(true))
            .unwrap_err();
        assert!(!matches!(err, AcpAgentError::Busy), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(15));
        let pid = f.pids()[0];
        assert!(wait_until(5, || !pid_alive(pid)), "leader must be reaped");
        assert!(
            wait_until(3, || m.helper_threads_alive() == 0),
            "helper threads must exit"
        );
    }

    #[test]
    fn t29_a_permission_request_flood_against_an_unread_stdin_retires_cleanly() {
        let f = Fake::new("t29");
        let m = AcpAgentManager::new();
        let mut cfg = f.cfg(&["--perm-flood", "20000"]);
        cfg.turn_timeout = Duration::from_secs(10);
        let started = Instant::now();
        let result = m.prompt(&cfg, "hi", opts(true));
        assert!(result.is_err(), "a flooding agent must not produce a reply");
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "no deadlock allowed"
        );
        let pid = f.pids()[0];
        assert!(wait_until(5, || !pid_alive(pid)));
    }

    #[test]
    fn t30_a_descendant_holding_the_pipes_cannot_keep_helper_threads_alive() {
        let f = Fake::new("t30");
        let m = AcpAgentManager::new();
        let cfg = f.cfg(&["--escaper"]);
        m.prompt(&cfg, "hi", opts(true)).unwrap();
        let leader = f.pids()[0];
        let escaper = f.gpid().expect("escaper pid");
        assert!(pid_alive(escaper));
        m.shutdown();
        assert!(
            wait_until(5, || !pid_alive(leader)),
            "leader must be reaped"
        );
        assert!(
            wait_until(5, || m.helper_threads_alive() == 0),
            "helpers must exit even though a descendant still holds the pipes"
        );
        assert!(
            pid_alive(escaper),
            "premise: the escaper survives the group kill and still holds the pipes"
        );
    }

    // ---------------------------------------------------------------- T33

    #[test]
    fn t33_a_new_manager_after_restart_starts_a_fresh_session() {
        let f = Fake::new("t33");
        let cfg = f.cfg(&["--memory"]);
        {
            let m = AcpAgentManager::new();
            assert_eq!(
                m.prompt(&cfg, "remember apple", opts(true)).unwrap().text,
                "ok"
            );
        }
        let m2 = AcpAgentManager::new();
        assert_eq!(
            m2.prompt(&cfg, "recall", opts(true)).unwrap().text,
            "secret=none"
        );
        assert_eq!(f.recv("session/new").len(), 2);
    }
}

#[cfg(not(unix))]
mod non_unix_tests {
    use crate::managers::acp_agent::*;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn spawning_is_unsupported_off_unix() {
        let cfg = AcpAgentConfig {
            command: PathBuf::from("agent"),
            args: Vec::new(),
            cwd: PathBuf::from("."),
            turn_timeout: Duration::from_secs(1),
            handshake_timeout: Duration::from_secs(1),
            cancel_grace: Duration::from_secs(1),
        };
        let m = AcpAgentManager::new();
        let err = m
            .prompt(
                &cfg,
                "hi",
                PromptOptions {
                    reuse_session: true,
                    expected_epoch: None,
                },
            )
            .unwrap_err();
        assert!(matches!(err, AcpAgentError::Unsupported), "{err:?}");
    }
}
