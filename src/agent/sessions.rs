use crate::sync::Mutex;
/// Session management for Genesis agent
///
/// Persistent sessions, rewind/undo, forking,
/// teleport (transfer between devices), resume.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SessionState {
    Active,
    Paused,
    Completed,
    Archived,
    Transferred, // Teleported to another device
}

#[derive(Clone, Copy)]
struct Checkpoint {
    id: u32,
    session_id: u32,
    turn_number: u32,
    timestamp: u64,
    file_snapshot_hash: u64, // Hash of file state at this point
    context_hash: u64,
    description_hash: u64,
}

#[derive(Clone, Copy)]
struct Session {
    id: u32,
    created_at: u64,
    last_active: u64,
    state: SessionState,
    turn_count: u32,
    total_tokens: u64,
    total_tool_calls: u32,
    parent_session: u32, // 0 = root session, nonzero = forked from
    pr_link_hash: u64,   // Linked GitHub PR (hash of URL)
    title_hash: u64,
    auto_title: bool,
}

#[derive(Clone, Copy)]
struct FileChange {
    session_id: u32,
    turn_number: u32,
    file_path_hash: u64,
    old_content_hash: u64,
    new_content_hash: u64,
    timestamp: u64,
    change_type: ChangeType,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ChangeType {
    Created,
    Modified,
    Deleted,
}

struct SessionManager {
    sessions: Vec<Session>,
    checkpoints: Vec<Checkpoint>,
    file_changes: Vec<FileChange>,
    next_session_id: u32,
    next_checkpoint_id: u32,
    active_session: u32,
    max_sessions: u32,
    auto_checkpoint: bool,    // Auto-checkpoint every N turns
    checkpoint_interval: u32, // Turns between auto-checkpoints
}

static SESSION_MGR: Mutex<Option<SessionManager>> = Mutex::new(None);

impl SessionManager {
    fn new() -> Self {
        SessionManager {
            sessions: Vec::new(),
            checkpoints: Vec::new(),
            file_changes: Vec::new(),
            next_session_id: 1,
            next_checkpoint_id: 1,
            active_session: 0,
            max_sessions: 100,
            auto_checkpoint: true,
            checkpoint_interval: 10,
        }
    }

    fn create_session(&mut self, timestamp: u64) -> u32 {
        let id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);
        self.sessions.push(Session {
            id,
            created_at: timestamp,
            last_active: timestamp,
            state: SessionState::Active,
            turn_count: 0,
            total_tokens: 0,
            total_tool_calls: 0,
            parent_session: 0,
            pr_link_hash: 0,
            title_hash: 0,
            auto_title: true,
        });
        self.active_session = id;
        id
    }

    fn fork_session(&mut self, source_id: u32, timestamp: u64) -> u32 {
        let id = self.create_session(timestamp);
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.parent_session = source_id;
        }
        // Copy checkpoints from source
        let src_checkpoints: Vec<Checkpoint> = self
            .checkpoints
            .iter()
            .filter(|c| c.session_id == source_id)
            .copied()
            .collect();
        for mut cp in src_checkpoints {
            cp.session_id = id;
            self.checkpoints.push(cp);
        }
        id
    }

    fn create_checkpoint(
        &mut self,
        session_id: u32,
        turn: u32,
        file_hash: u64,
        timestamp: u64,
    ) -> u32 {
        let id = self.next_checkpoint_id;
        self.next_checkpoint_id = self.next_checkpoint_id.saturating_add(1);
        self.checkpoints.push(Checkpoint {
            id,
            session_id,
            turn_number: turn,
            timestamp,
            file_snapshot_hash: file_hash,
            context_hash: 0,
            description_hash: 0,
        });
        id
    }

    fn record_file_change(
        &mut self,
        session_id: u32,
        turn: u32,
        path_hash: u64,
        old_hash: u64,
        new_hash: u64,
        change_type: ChangeType,
        timestamp: u64,
    ) {
        self.file_changes.push(FileChange {
            session_id,
            turn_number: turn,
            file_path_hash: path_hash,
            old_content_hash: old_hash,
            new_content_hash: new_hash,
            timestamp,
            change_type,
        });
    }

    /// Rewind to a checkpoint — undo all changes after that point
    fn rewind_to(&mut self, checkpoint_id: u32) -> Vec<FileChange> {
        let checkpoint = self.checkpoints.iter().find(|c| c.id == checkpoint_id);
        if let Some(cp) = checkpoint {
            let session_id = cp.session_id;
            let turn = cp.turn_number;
            // Collect all changes after this checkpoint (in reverse order for undo)
            let mut changes: Vec<FileChange> = self
                .file_changes
                .iter()
                .filter(|fc| fc.session_id == session_id && fc.turn_number > turn)
                .copied()
                .collect();
            changes.reverse();
            // Remove those changes from history
            self.file_changes
                .retain(|fc| !(fc.session_id == session_id && fc.turn_number > turn));
            changes
        } else {
            Vec::new()
        }
    }

    /// Teleport — prepare session for transfer to another device
    fn teleport(&mut self, session_id: u32) -> Option<u64> {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            s.state = SessionState::Transferred;
            // Return a transfer token (hash of session state)
            Some(session_id as u64 * 0x5AFE_CAFE + s.total_tokens)
        } else {
            None
        }
    }

    fn resume_session(&mut self, session_id: u32, timestamp: u64) -> bool {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            s.state = SessionState::Active;
            s.last_active = timestamp;
            self.active_session = session_id;
            true
        } else {
            false
        }
    }

    fn link_pr(&mut self, session_id: u32, pr_hash: u64) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            s.pr_link_hash = pr_hash;
        }
    }

    fn get_recent_sessions(&self, limit: usize) -> Vec<u32> {
        let mut sorted: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| s.state != SessionState::Archived)
            .collect();
        sorted.sort_by(|a, b| b.last_active.cmp(&a.last_active));
        sorted.iter().take(limit).map(|s| s.id).collect()
    }

    fn should_auto_checkpoint(&self, session_id: u32) -> bool {
        if !self.auto_checkpoint {
            return false;
        }
        if let Some(s) = self.sessions.iter().find(|s| s.id == session_id) {
            let last_cp = self
                .checkpoints
                .iter()
                .filter(|c| c.session_id == session_id)
                .max_by_key(|c| c.turn_number)
                .map_or(0, |c| c.turn_number);
            s.turn_count - last_cp >= self.checkpoint_interval
        } else {
            false
        }
    }
}

pub fn init() {
    let mut sm = SESSION_MGR.lock();
    *sm = Some(SessionManager::new());
    serial_println!("    Sessions: rewind, fork, teleport, auto-checkpoint ready");
}
