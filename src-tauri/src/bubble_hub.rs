//! Pure core of the bubble stack (DESIGN.md A7-2, A8-2, A9-1).
//!
//! No Tauri types, no I/O and no clock reads: the caller passes `now` in milliseconds. Admission
//! checks a line's stamp against the chat log's admission tuple (`GateState`), so a line said
//! under an older session or recording epoch is refused. The projection is the only thing shown:
//! it is empty unless every gate is open, and `publish` emits a new revision whenever the
//! projection changes, so suppression and restoration are both visible to the page.
//!
//! Speech scopes: while a reply is being spoken, the assistant lines admitted under its scope
//! are held (alive up to `HOLD_MAX_MS`). `speech_end` releases them with a short `LINGER_MS`.
//! A scope whose end never arrives is forgotten after `HOLD_MAX_MS`.
#![cfg_attr(not(test), allow(dead_code))]

use crate::managers::chat_log::{GateState, Stamp};

/// Most bubbles shown at once. The oldest is dropped when a newer line arrives.
pub const MAX_ITEMS: usize = 3;
/// Fade-out at the end of a bubble's lifetime, in milliseconds.
pub const FADE_MS: u64 = 600;
/// Longest a held assistant line stays up, and the age after which a speech scope whose end was
/// never received is forgotten. A deliberate cap: a lost signal must not pin a bubble forever.
pub const HOLD_MAX_MS: u64 = 120_000;
/// How long a held line stays after its speech scope ends, in milliseconds.
pub const LINGER_MS: u64 = 2_000;

/// Longest display text kept, in characters (ellipsis included).
const DISPLAY_CAP: usize = 240;
const TTL_BASE_MS: u64 = 3_500;
const TTL_PER_CHAR_MS: u64 = 45;
const TTL_MIN_MS: u64 = 4_000;
const TTL_MAX_MS: u64 = 12_000;

/// Lifetime of a bubble for a text of `chars` characters: `clamp(3500 + 45 * chars, 4000, 12000)`.
pub fn ttl_ms(chars: usize) -> u64 {
    let chars = u64::try_from(chars).unwrap_or(u64::MAX);
    TTL_BASE_MS
        .saturating_add(TTL_PER_CHAR_MS.saturating_mul(chars))
        .clamp(TTL_MIN_MS, TTL_MAX_MS)
}

/// The text a bubble shows: whitespace collapsed and trimmed, at most 240 characters. A longer
/// text is cut at a word boundary and ends with an ellipsis. The result is idempotent.
pub fn cap_display_text(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<&str>>().join(" ");
    if collapsed.chars().count() <= DISPLAY_CAP {
        return collapsed;
    }
    let chars: Vec<char> = collapsed.chars().collect();
    // One character is reserved for the ellipsis.
    let budget = DISPLAY_CAP - 1;
    let mut cut = budget;
    let at_boundary = chars.get(budget).is_some_and(|c| c.is_whitespace());
    if !at_boundary {
        if let Some(space) = chars.iter().take(budget).rposition(|c| *c == ' ') {
            if space > 0 {
                cut = space;
            }
        }
    }
    let body: String = chars.iter().take(cut).collect();
    format!("{}…", body.trim_end())
}

/// Who said the line shown in a bubble.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BubbleRole {
    /// The person speaking to Vox (right aligned).
    User,
    /// Vox (left aligned).
    Assistant,
}

/// One transcript line offered to the hub, with the stamp it was said under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubEntry {
    /// The line's sequence id. Ids not greater than the highest seen are ignored.
    pub id: u64,
    /// Speaker.
    pub role: BubbleRole,
    /// Raw text; the hub caps it for display.
    pub text: String,
    /// Admission tuple taken when the line was said.
    pub stamp: Stamp,
}

/// A bubble as projected for the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjItem {
    /// The line's id.
    pub id: u64,
    /// Speaker.
    pub role: BubbleRole,
    /// Display text (already capped).
    pub text: String,
    /// Milliseconds since the line was admitted, at projection time.
    pub age_ms: u64,
    /// Lifetime in milliseconds.
    pub ttl_ms: u64,
}

/// A revision of the projection, as sent to the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Window instance the snapshot is for.
    pub instance: u64,
    /// Revision; grows whenever the projection changes.
    pub rev: u64,
    /// The visible bubbles, oldest first.
    pub items: Vec<ProjItem>,
}

/// A stored bubble, with its admission time.
#[derive(Debug, Clone)]
struct Stored {
    id: u64,
    role: BubbleRole,
    text: String,
    born_ms: u64,
    ttl_ms: u64,
    /// The speech scope holding this line, while it is held.
    scope: Option<u64>,
    /// Whether the line is held by `scope` (lifetime `HOLD_MAX_MS` until released).
    hold: bool,
}

impl Stored {
    /// Alive strictly before `born + ttl`.
    fn alive(&self, now: u64) -> bool {
        now < self.born_ms.saturating_add(self.ttl_ms)
    }
}

/// The bubble state machine. Owned by the native shell behind one lock.
#[derive(Debug)]
pub struct BubbleHub {
    recording: bool,
    armed: bool,
    orb_visible: bool,
    classic_visible: bool,
    placement_ok: bool,
    ready: bool,
    instance: u64,
    rev: u64,
    highest_id: u64,
    items: Vec<Stored>,
    /// Active speech scopes as (scope id, started at ms), oldest first.
    scopes: Vec<(u64, u64)>,
    published: Vec<ProjItem>,
}

impl BubbleHub {
    /// A hub mirroring the chat-log recording flag. Not armed, not ready, nothing shown.
    pub fn new(recording: bool) -> BubbleHub {
        BubbleHub {
            recording,
            armed: false,
            orb_visible: false,
            classic_visible: false,
            placement_ok: false,
            ready: false,
            instance: 0,
            rev: 0,
            highest_id: 0,
            items: Vec::new(),
            scopes: Vec::new(),
            published: Vec::new(),
        }
    }

    /// The recording flag changed. Turning it off clears every bubble at once, and turning it
    /// back on never restores them.
    pub fn set_recording(&mut self, on: bool) {
        self.recording = on;
        if !on {
            self.items.clear();
        }
    }

    /// Armed by a session that surfaces as bubbles, disarmed otherwise.
    pub fn set_armed(&mut self, on: bool) {
        self.armed = on;
    }

    /// Whether the orb is visible.
    pub fn set_orb_visible(&mut self, on: bool) {
        self.orb_visible = on;
    }

    /// Whether the classic conversation window is visible.
    pub fn set_classic_visible(&mut self, on: bool) {
        self.classic_visible = on;
    }

    /// Whether a placement exists for the current orb position.
    pub fn set_placement_ok(&mut self, on: bool) {
        self.placement_ok = on;
    }

    /// The page for `instance` is ready. Ignored for any other instance.
    pub fn set_ready(&mut self, instance: u64) {
        if instance == self.instance {
            self.ready = true;
        }
    }

    /// Whether the page of the current instance has reported ready.
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// The current window instance (0 before the first window).
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// A new window was created: a new instance, not ready, with no published state yet.
    /// Stored lines are kept and shown again once the new page is ready.
    pub fn new_instance(&mut self) -> u64 {
        self.instance = self.instance.saturating_add(1);
        self.ready = false;
        self.published.clear();
        self.instance
    }

    /// Session reset: every bubble and every speech scope is cleared.
    pub fn on_reset(&mut self) {
        self.items.clear();
        self.scopes.clear();
    }

    /// The user cleared the transcript: every bubble is cleared.
    pub fn on_clear(&mut self) {
        self.items.clear();
    }

    /// Offers one transcript line. Admitted only while the hub is recording and armed, and only
    /// when the line's stamp equals the current tuple (session token, recording epoch, and
    /// recording on). A refused line changes nothing, including the highest id seen. Returns
    /// whether the line was admitted.
    pub fn on_entry(&mut self, entry: HubEntry, gate: GateState, now: u64) -> bool {
        self.prune(now);
        if !self.recording || !self.armed {
            return false;
        }
        if !gate.recording
            || gate.session_token != entry.stamp.session_token
            || gate.rec_epoch != entry.stamp.rec_epoch
        {
            return false;
        }
        if entry.id <= self.highest_id {
            return false;
        }
        let text = cap_display_text(&entry.text);
        if text.is_empty() {
            return false;
        }
        self.highest_id = entry.id;
        // An assistant line said while a reply is being spoken is held by the newest active scope.
        let held_by = match entry.role {
            BubbleRole::Assistant => self.scopes.last().map(|(id, _)| *id),
            BubbleRole::User => None,
        };
        let (ttl, scope, hold) = match held_by {
            Some(id) => (HOLD_MAX_MS, Some(id), true),
            None => (ttl_ms(text.chars().count()), None, false),
        };
        self.items.push(Stored {
            id: entry.id,
            role: entry.role,
            text,
            born_ms: now,
            ttl_ms: ttl,
            scope,
            hold,
        });
        let excess = self.items.len().saturating_sub(MAX_ITEMS);
        self.items.drain(..excess);
        true
    }

    /// A reply started being spoken under `scope`. Admitted exactly like a line: refused (returns
    /// false, changes nothing) unless the hub is recording and armed and the stamp matches the
    /// gate. A repeated begin of an active scope changes nothing and returns true.
    pub fn speech_begin(&mut self, scope: u64, stamp: Stamp, gate: GateState, now: u64) -> bool {
        self.prune(now);
        if !self.recording || !self.armed {
            return false;
        }
        if !gate.recording
            || gate.session_token != stamp.session_token
            || gate.rec_epoch != stamp.rec_epoch
        {
            return false;
        }
        if !self.scopes.iter().any(|(id, _)| *id == scope) {
            self.scopes.push((scope, now));
        }
        true
    }

    /// The reply under `scope` stopped being spoken. The scope is removed and every line it held
    /// is released: reborn at `now` with the short `LINGER_MS` lifetime. An unknown scope changes
    /// nothing. Lines already pruned are not brought back.
    pub fn speech_end(&mut self, scope: u64, now: u64) {
        self.prune(now);
        let before = self.scopes.len();
        self.scopes.retain(|(id, _)| *id != scope);
        if self.scopes.len() == before {
            return;
        }
        for item in self.items.iter_mut() {
            if item.hold && item.scope == Some(scope) {
                item.born_ms = now;
                item.ttl_ms = LINGER_MS;
                item.hold = false;
                item.scope = None;
            }
        }
    }

    /// Removes bubbles whose lifetime has ended, and speech scopes whose end never came within
    /// `HOLD_MAX_MS` of their begin.
    pub fn prune(&mut self, now: u64) {
        self.items.retain(|item| item.alive(now));
        self.scopes
            .retain(|(_, started)| now < started.saturating_add(HOLD_MAX_MS));
    }

    /// What the page should show at `now`: at most `MAX_ITEMS` bubbles, oldest first, and only
    /// while every gate is open. Otherwise empty.
    pub fn projection(&self, now: u64) -> Vec<ProjItem> {
        if !self.gates_open() {
            return Vec::new();
        }
        self.items
            .iter()
            .filter(|item| item.alive(now))
            .map(|item| ProjItem {
                id: item.id,
                role: item.role,
                text: item.text.clone(),
                age_ms: now.saturating_sub(item.born_ms),
                ttl_ms: item.ttl_ms,
            })
            .collect()
    }

    /// Every gate that must be open for anything to show.
    fn gates_open(&self) -> bool {
        self.armed
            && self.recording
            && self.ready
            && self.orb_visible
            && !self.classic_visible
            && self.placement_ok
    }

    /// Prunes, then bumps the revision and remembers the projection when it changed. Returns
    /// whether it changed.
    fn refresh(&mut self, now: u64) -> bool {
        self.prune(now);
        let current = self.projection(now);
        if same_items(&current, &self.published) {
            return false;
        }
        self.rev = self.rev.saturating_add(1);
        self.published = current;
        true
    }

    /// The snapshot to send when the projection changed at `now`, or `None` when it did not.
    pub fn publish(&mut self, now: u64) -> Option<Snapshot> {
        if !self.refresh(now) {
            return None;
        }
        Some(self.snapshot(now))
    }

    /// The current snapshot for `instance`, for a readiness request. `None` for any other
    /// instance. Re-sends the same revision when nothing changed.
    pub fn snapshot_for_ready(&mut self, instance: u64, now: u64) -> Option<Snapshot> {
        if instance != self.instance {
            return None;
        }
        self.refresh(now);
        Some(self.snapshot(now))
    }

    /// The current revision and projection at `now`, without publishing. `None` before any
    /// window instance exists.
    pub fn current(&self, now: u64) -> Option<Snapshot> {
        (self.instance != 0).then(|| self.snapshot(now))
    }

    /// Current revision with the projection at `now`.
    fn snapshot(&self, now: u64) -> Snapshot {
        Snapshot {
            instance: self.instance,
            rev: self.rev,
            items: self.projection(now),
        }
    }
}

/// Equal when the visible lines match. Ages are ignored: they change with time alone.
fn same_items(a: &[ProjItem], b: &[ProjItem]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.iter()).all(|(x, y)| {
            x.id == y.id && x.role == y.role && x.text == y.text && x.ttl_ms == y.ttl_ms
        })
}
