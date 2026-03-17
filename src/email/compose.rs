use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// Email compose module for Genesis
///
/// Handles creating, editing, saving, and sending email drafts.
/// Supports reply, reply-all, forward, attachments, HTML composition,
/// signatures, and auto-save.
use alloc::vec::Vec;

// ── Constants ──────────────────────────────────────────────────────────

/// Maximum number of drafts stored simultaneously
const MAX_DRAFTS: usize = 100;

/// Maximum recipients per draft (To + Cc + Bcc)
const MAX_RECIPIENTS_TOTAL: usize = 200;

/// Maximum attachments per draft
const MAX_ATTACHMENTS: usize = 25;

/// Auto-save interval in seconds
const AUTO_SAVE_INTERVAL: u64 = 30;

/// Maximum subject length (in hash representation, always 1 u64)
const MAX_SUBJECT_LEN: usize = 256;

/// Maximum signature pool size
const MAX_SIGNATURES: usize = 10;

// ── Types ──────────────────────────────────────────────────────────────

/// Draft composition state
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DraftState {
    /// Newly created, not yet modified
    New,
    /// User has made changes
    Editing,
    /// Draft has been auto-saved or manually saved
    Saved,
    /// Draft is being sent
    Sending,
    /// Successfully sent
    Sent,
    /// Send failed
    Failed,
    /// Draft was discarded
    Discarded,
}

/// How this draft was initiated
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ComposeMode {
    /// Brand new message
    New,
    /// Reply to a single sender
    Reply,
    /// Reply to all recipients
    ReplyAll,
    /// Forward an existing message
    Forward,
    /// Editing a previously saved draft
    EditDraft,
    /// Resend a bounced message
    Resend,
}

/// Attachment descriptor for compose
#[derive(Clone, Copy, Debug)]
pub struct ComposeAttachment {
    /// Unique attachment identifier
    pub id: u32,
    /// Filename hash
    pub name_hash: u64,
    /// MIME type hash
    pub mime_hash: u64,
    /// Content hash
    pub content_hash: u64,
    /// Size in bytes
    pub size: u32,
    /// Whether this is an inline attachment (e.g. embedded image)
    pub inline: bool,
    /// Content-ID hash for inline references
    pub content_id_hash: u64,
}

/// Email signature
#[derive(Clone, Copy, Debug)]
pub struct Signature {
    /// Signature identifier
    pub id: u32,
    /// Signature content hash (plaintext)
    pub text_hash: u64,
    /// Signature content hash (HTML version)
    pub html_hash: u64,
    /// Whether this is the default signature
    pub is_default: bool,
    /// Account identifier this signature belongs to
    pub account_id: u32,
}

/// An email draft in composition
#[derive(Clone, Debug)]
pub struct Draft {
    /// Unique draft identifier
    pub id: u64,
    /// To recipients (address hashes)
    pub to_list: Vec<u64>,
    /// Cc recipients (address hashes)
    pub cc_list: Vec<u64>,
    /// Bcc recipients (address hashes)
    pub bcc_list: Vec<u64>,
    /// Subject line hash
    pub subject_hash: u64,
    /// Body content hash
    pub body_hash: u64,
    /// Attached files
    pub attachments: Vec<ComposeAttachment>,
    /// Message ID hash of the message being replied to (0 = not a reply)
    pub reply_to: u64,
    /// Whether the body is HTML format
    pub is_html: bool,
    /// Signature hash to append (0 = no signature)
    pub signature_hash: u64,
    /// Timestamp of the last auto-save
    pub auto_save_time: u64,
    /// Current draft state
    pub state: DraftState,
    /// How this draft was created
    pub mode: ComposeMode,
    /// Sender address hash (from account)
    pub from_hash: u64,
    /// Account ID sending from
    pub account_id: u32,
    /// Thread ID for conversation threading
    pub thread_id: u64,
    /// Priority (1=high, 3=normal, 5=low)
    pub priority: u8,
    /// Whether to request a read receipt
    pub read_receipt: bool,
    /// Timestamp of draft creation
    pub created_at: u64,
    /// Timestamp of last modification
    pub modified_at: u64,
    /// Number of times this draft has been saved
    pub save_count: u32,
}

// ── Global State ───────────────────────────────────────────────────────

static DRAFTS: Mutex<Option<Vec<Draft>>> = Mutex::new(None);
static SIGNATURES: Mutex<Option<Vec<Signature>>> = Mutex::new(None);
static NEXT_DRAFT_ID: Mutex<u64> = Mutex::new(1);
static NEXT_ATTACH_ID: Mutex<u32> = Mutex::new(1);

// ── Helper Functions ───────────────────────────────────────────────────

/// Allocate the next unique draft ID
fn alloc_draft_id() -> u64 {
    let mut id = NEXT_DRAFT_ID.lock();
    let current = *id;
    *id = current.wrapping_add(1);
    current
}

/// Allocate the next unique attachment ID
fn alloc_attach_id() -> u32 {
    let mut id = NEXT_ATTACH_ID.lock();
    let current = *id;
    *id = current.wrapping_add(1);
    current
}

/// Count total recipients in a draft
fn total_recipients(draft: &Draft) -> usize {
    draft.to_list.len() + draft.cc_list.len() + draft.bcc_list.len()
}

/// Compose hash for generating derived message IDs
fn compose_hash(a: u64, b: u64) -> u64 {
    a.wrapping_mul(0x517CC1B727220A95).wrapping_add(b)
}

/// Get the default signature for an account
fn get_default_signature(account_id: u32) -> u64 {
    let sigs = SIGNATURES.lock();
    if let Some(ref list) = *sigs {
        for sig in list.iter() {
            if sig.account_id == account_id && sig.is_default {
                return sig.text_hash;
            }
        }
    }
    0
}

// ── Core Compose Operations ───────────────────────────────────────────

/// Create a new empty draft
pub fn new_draft(from_hash: u64, account_id: u32) -> u64 {
    let id = alloc_draft_id();
    let sig = get_default_signature(account_id);

    let draft = Draft {
        id,
        to_list: Vec::new(),
        cc_list: Vec::new(),
        bcc_list: Vec::new(),
        subject_hash: 0,
        body_hash: 0,
        attachments: Vec::new(),
        reply_to: 0,
        is_html: false,
        signature_hash: sig,
        auto_save_time: 0,
        state: DraftState::New,
        mode: ComposeMode::New,
        from_hash,
        account_id,
        thread_id: 0,
        priority: 3,
        read_receipt: false,
        created_at: 0,
        modified_at: 0,
        save_count: 0,
    };

    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if list.len() >= MAX_DRAFTS {
            // Remove oldest discarded or sent draft
            let discard_pos = list
                .iter()
                .position(|d| d.state == DraftState::Discarded || d.state == DraftState::Sent);
            if let Some(pos) = discard_pos {
                list.remove(pos);
            } else {
                // Remove oldest draft
                list.remove(0);
            }
        }
        list.push(draft);
    }

    serial_println!("[compose] Created new draft id={}", id);
    id
}

/// Set the recipients on a draft (To list)
pub fn set_recipients(draft_id: u64, to_list: Vec<u64>) -> bool {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            if to_list.len() + draft.cc_list.len() + draft.bcc_list.len() > MAX_RECIPIENTS_TOTAL {
                serial_println!("[compose] Too many recipients for draft {}", draft_id);
                return false;
            }
            draft.to_list = to_list;
            draft.state = DraftState::Editing;
            return true;
        }
    }
    false
}

/// Set Cc recipients
pub fn set_cc(draft_id: u64, cc_list: Vec<u64>) -> bool {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            if draft.to_list.len() + cc_list.len() + draft.bcc_list.len() > MAX_RECIPIENTS_TOTAL {
                return false;
            }
            draft.cc_list = cc_list;
            draft.state = DraftState::Editing;
            return true;
        }
    }
    false
}

/// Set Bcc recipients
pub fn set_bcc(draft_id: u64, bcc_list: Vec<u64>) -> bool {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            if draft.to_list.len() + draft.cc_list.len() + bcc_list.len() > MAX_RECIPIENTS_TOTAL {
                return false;
            }
            draft.bcc_list = bcc_list;
            draft.state = DraftState::Editing;
            return true;
        }
    }
    false
}

/// Set the subject
pub fn set_subject(draft_id: u64, subject_hash: u64) -> bool {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            draft.subject_hash = subject_hash;
            draft.state = DraftState::Editing;
            return true;
        }
    }
    false
}

/// Set the body content
pub fn set_body(draft_id: u64, body_hash: u64, is_html: bool) -> bool {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            draft.body_hash = body_hash;
            draft.is_html = is_html;
            draft.state = DraftState::Editing;
            return true;
        }
    }
    false
}

/// Set the priority
pub fn set_priority(draft_id: u64, priority: u8) -> bool {
    if priority < 1 || priority > 5 {
        return false;
    }
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            draft.priority = priority;
            draft.state = DraftState::Editing;
            return true;
        }
    }
    false
}

/// Attach a file to the draft
pub fn attach_file(
    draft_id: u64,
    name_hash: u64,
    mime_hash: u64,
    content_hash: u64,
    size: u32,
) -> Option<u32> {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            if draft.attachments.len() >= MAX_ATTACHMENTS {
                serial_println!("[compose] Max attachments reached for draft {}", draft_id);
                return None;
            }

            let attach_id = alloc_attach_id();
            draft.attachments.push(ComposeAttachment {
                id: attach_id,
                name_hash,
                mime_hash,
                content_hash,
                size,
                inline: false,
                content_id_hash: 0,
            });
            draft.state = DraftState::Editing;

            serial_println!(
                "[compose] Attached file {} to draft {} ({} bytes)",
                attach_id,
                draft_id,
                size
            );
            return Some(attach_id);
        }
    }
    None
}

/// Remove an attachment from a draft
pub fn remove_attachment(draft_id: u64, attach_id: u32) -> bool {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            if let Some(pos) = draft.attachments.iter().position(|a| a.id == attach_id) {
                draft.attachments.remove(pos);
                draft.state = DraftState::Editing;
                return true;
            }
        }
    }
    false
}

/// Save the draft to storage
pub fn save_draft(draft_id: u64) -> bool {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            draft.state = DraftState::Saved;
            draft.save_count = draft.save_count.saturating_add(1);
            serial_println!(
                "[compose] Draft {} saved (save #{})",
                draft_id,
                draft.save_count
            );
            return true;
        }
    }
    false
}

/// Auto-save the draft (called periodically)
pub fn auto_save(draft_id: u64, current_time: u64) -> bool {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            if draft.state == DraftState::Editing {
                let elapsed = current_time.wrapping_sub(draft.auto_save_time);
                if elapsed >= AUTO_SAVE_INTERVAL || draft.auto_save_time == 0 {
                    draft.auto_save_time = current_time;
                    draft.state = DraftState::Saved;
                    draft.save_count = draft.save_count.saturating_add(1);
                    serial_println!("[compose] Auto-saved draft {}", draft_id);
                    return true;
                }
            }
        }
    }
    false
}

/// Send the draft as an email
///
/// Validates the draft has at least one recipient and a body,
/// then transitions to Sending state. The actual SMTP send is
/// handled by the smtp_client module.
pub fn send(draft_id: u64) -> Result<u64, u8> {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            // Validate
            if draft.to_list.is_empty() && draft.cc_list.is_empty() && draft.bcc_list.is_empty() {
                serial_println!("[compose] Cannot send draft {} — no recipients", draft_id);
                return Err(1); // no recipients
            }
            if draft.body_hash == 0 && draft.subject_hash == 0 {
                serial_println!("[compose] Cannot send draft {} — empty message", draft_id);
                return Err(2); // empty message
            }
            if draft.from_hash == 0 {
                serial_println!("[compose] Cannot send draft {} — no sender", draft_id);
                return Err(3); // no sender
            }

            draft.state = DraftState::Sending;

            // Generate a message ID for tracking
            let message_id = compose_hash(draft.id, draft.from_hash);

            // Mark as sent (actual SMTP send would happen here)
            draft.state = DraftState::Sent;
            serial_println!(
                "[compose] Draft {} sent (message_id=0x{:016X})",
                draft_id,
                message_id
            );

            return Ok(message_id);
        }
    }
    Err(4) // draft not found
}

/// Create a reply draft from an existing message
pub fn reply(
    original_msg_id: u64,
    original_from: u64,
    original_subject: u64,
    original_body: u64,
    my_from: u64,
    account_id: u32,
) -> u64 {
    let draft_id = new_draft(my_from, account_id);

    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            draft.to_list = vec![original_from];
            draft.reply_to = original_msg_id;
            draft.mode = ComposeMode::Reply;
            draft.thread_id = original_subject; // thread by subject

            // Prefix subject with "Re: " hash
            let re_prefix_hash: u64 = 0x00000000_00526520; // "Re: " as simple encoding
            draft.subject_hash = compose_hash(re_prefix_hash, original_subject);

            // Quote original body
            draft.body_hash = compose_hash(0xAABBCCDD_11223344, original_body);
        }
    }

    serial_println!(
        "[compose] Reply draft {} created for message 0x{:016X}",
        draft_id,
        original_msg_id
    );
    draft_id
}

/// Create a reply-all draft
pub fn reply_all(
    original_msg_id: u64,
    original_from: u64,
    original_to_list: Vec<u64>,
    original_cc_list: Vec<u64>,
    original_subject: u64,
    original_body: u64,
    my_from: u64,
    account_id: u32,
) -> u64 {
    let draft_id = new_draft(my_from, account_id);

    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            // To: original sender
            draft.to_list = vec![original_from];

            // Cc: all original To (except myself) + all original Cc
            let mut cc = Vec::new();
            for &addr in &original_to_list {
                if addr != my_from {
                    cc.push(addr);
                }
            }
            for &addr in &original_cc_list {
                if addr != my_from && addr != original_from {
                    cc.push(addr);
                }
            }
            draft.cc_list = cc;

            draft.reply_to = original_msg_id;
            draft.mode = ComposeMode::ReplyAll;
            draft.thread_id = original_subject;

            let re_prefix_hash: u64 = 0x00000000_00526520;
            draft.subject_hash = compose_hash(re_prefix_hash, original_subject);
            draft.body_hash = compose_hash(0xAABBCCDD_11223344, original_body);
        }
    }

    serial_println!(
        "[compose] Reply-all draft {} created for message 0x{:016X}",
        draft_id,
        original_msg_id
    );
    draft_id
}

/// Create a forward draft
pub fn forward(
    original_msg_id: u64,
    original_from: u64,
    original_subject: u64,
    original_body: u64,
    original_attachments: Vec<ComposeAttachment>,
    my_from: u64,
    account_id: u32,
) -> u64 {
    let draft_id = new_draft(my_from, account_id);

    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            draft.mode = ComposeMode::Forward;
            draft.reply_to = original_msg_id;

            // Prefix subject with "Fwd: " hash
            let fwd_prefix_hash: u64 = 0x00000000_46776420; // "Fwd:" as simple encoding
            draft.subject_hash = compose_hash(fwd_prefix_hash, original_subject);

            // Include forwarded body with attribution
            let attribution_hash = compose_hash(original_from, 0xFEDCBA98_76543210);
            draft.body_hash = compose_hash(attribution_hash, original_body);

            // Copy attachments from original
            for att in original_attachments {
                if draft.attachments.len() < MAX_ATTACHMENTS {
                    draft.attachments.push(att);
                }
            }
        }
    }

    serial_println!(
        "[compose] Forward draft {} created from message 0x{:016X}",
        draft_id,
        original_msg_id
    );
    draft_id
}

/// Get a draft by ID
pub fn get_draft(draft_id: u64) -> Option<Draft> {
    let drafts = DRAFTS.lock();
    if let Some(ref list) = *drafts {
        list.iter().find(|d| d.id == draft_id).cloned()
    } else {
        None
    }
}

/// List all active drafts (not sent or discarded)
pub fn list_drafts() -> Vec<Draft> {
    let drafts = DRAFTS.lock();
    if let Some(ref list) = *drafts {
        list.iter()
            .filter(|d| d.state != DraftState::Sent && d.state != DraftState::Discarded)
            .cloned()
            .collect()
    } else {
        Vec::new()
    }
}

/// Discard a draft
pub fn discard_draft(draft_id: u64) -> bool {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            draft.state = DraftState::Discarded;
            serial_println!("[compose] Draft {} discarded", draft_id);
            return true;
        }
    }
    false
}

/// Set the signature for a draft
pub fn set_signature(draft_id: u64, signature_hash: u64) -> bool {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            draft.signature_hash = signature_hash;
            draft.state = DraftState::Editing;
            return true;
        }
    }
    false
}

/// Add a signature to the signature pool
pub fn add_signature(
    text_hash: u64,
    html_hash: u64,
    account_id: u32,
    is_default: bool,
) -> Option<u32> {
    let mut sigs = SIGNATURES.lock();
    if let Some(ref mut list) = *sigs {
        if list.len() >= MAX_SIGNATURES {
            return None;
        }

        // If setting as default, unset existing defaults for this account
        if is_default {
            for sig in list.iter_mut() {
                if sig.account_id == account_id {
                    sig.is_default = false;
                }
            }
        }

        let sig_id = (list.len() as u32).wrapping_add(1);
        list.push(Signature {
            id: sig_id,
            text_hash,
            html_hash,
            is_default,
            account_id,
        });
        serial_println!(
            "[compose] Added signature {} (default={})",
            sig_id,
            is_default
        );
        Some(sig_id)
    } else {
        None
    }
}

/// Toggle read receipt request
pub fn set_read_receipt(draft_id: u64, enabled: bool) -> bool {
    let mut drafts = DRAFTS.lock();
    if let Some(ref mut list) = *drafts {
        if let Some(draft) = list.iter_mut().find(|d| d.id == draft_id) {
            draft.read_receipt = enabled;
            return true;
        }
    }
    false
}

/// Get the count of active (non-sent, non-discarded) drafts
pub fn active_draft_count() -> usize {
    let drafts = DRAFTS.lock();
    if let Some(ref list) = *drafts {
        list.iter()
            .filter(|d| d.state != DraftState::Sent && d.state != DraftState::Discarded)
            .count()
    } else {
        0
    }
}

/// Initialize the compose subsystem
pub fn init() {
    {
        let mut drafts = DRAFTS.lock();
        *drafts = Some(Vec::new());
    }
    {
        let mut sigs = SIGNATURES.lock();
        *sigs = Some(Vec::new());
    }
    serial_println!(
        "[compose] Compose engine initialized (max {} drafts, auto-save {}s)",
        MAX_DRAFTS,
        AUTO_SAVE_INTERVAL
    );
}
