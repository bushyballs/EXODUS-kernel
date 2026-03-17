use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Email inbox manager for Genesis
///
/// Local storage and organization of email messages. Handles folder
/// management, search, sorting, flag operations, and unread counts.
///
/// Messages are stored locally after being fetched from IMAP. The inbox
/// manager provides the UI-facing API for browsing and organizing email.
use alloc::vec::Vec;

// ── Constants ──────────────────────────────────────────────────────────

/// Maximum number of messages stored locally
const MAX_MESSAGES: usize = 10000;

/// Maximum folders (system + custom)
const MAX_FOLDERS: usize = 64;

/// Maximum custom folder name hash slots
const MAX_CUSTOM_FOLDERS: usize = 48;

/// Default page size for paginated queries
const DEFAULT_PAGE_SIZE: usize = 50;

/// Search result limit
const MAX_SEARCH_RESULTS: usize = 200;

// ── Types ──────────────────────────────────────────────────────────────

/// Standard and custom email folders
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Folder {
    /// Primary inbox
    Inbox,
    /// Sent messages
    Sent,
    /// Work-in-progress drafts
    Drafts,
    /// Deleted messages (before permanent removal)
    Trash,
    /// Spam / junk
    Spam,
    /// Archived messages
    Archive,
    /// Starred / important
    Starred,
    /// All mail (virtual folder)
    AllMail,
    /// User-defined folder identified by hash
    Custom(u64),
}

/// Sort criteria for message listing
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SortBy {
    /// Most recent first (default)
    DateDesc,
    /// Oldest first
    DateAsc,
    /// By sender (alphabetical by hash)
    SenderAsc,
    /// By sender (reverse)
    SenderDesc,
    /// By subject (alphabetical by hash)
    SubjectAsc,
    /// By subject (reverse)
    SubjectDesc,
    /// Largest first
    SizeDesc,
    /// Smallest first
    SizeAsc,
    /// Unread first, then by date
    UnreadFirst,
    /// Flagged first, then by date
    FlaggedFirst,
}

/// Search field specifier
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SearchField {
    /// Search all fields
    All,
    /// Search sender only
    From,
    /// Search recipient only
    To,
    /// Search subject only
    Subject,
    /// Search body only
    Body,
    /// Search messages with attachments
    HasAttachment,
    /// Search by date range
    DateRange,
}

/// A locally stored email message
#[derive(Clone, Copy, Debug)]
pub struct EmailMessage {
    /// Unique message identifier (UID from IMAP or locally generated)
    pub id: u64,
    /// Hash of the sender address
    pub from: u64,
    /// Hash of the primary recipient
    pub to: u64,
    /// Hash of the subject line
    pub subject_hash: u64,
    /// Hash of the message body
    pub body_hash: u64,
    /// Timestamp (Unix epoch seconds)
    pub date: u64,
    /// Whether the message has been read
    pub read: bool,
    /// Whether the message is flagged/starred
    pub flagged: bool,
    /// Which folder this message is in
    pub folder: Folder,
    /// Whether the message has attachments
    pub has_attachments: bool,
    /// Message size in bytes
    pub size: u32,
    /// Thread/conversation identifier hash
    pub thread_id: u64,
    /// Hash of the In-Reply-To message ID
    pub reply_to_id: u64,
    /// Number of Cc recipients
    pub cc_count: u8,
    /// Whether this message has been answered
    pub answered: bool,
    /// Whether this message is a draft
    pub is_draft: bool,
    /// Priority level (1=high, 3=normal, 5=low)
    pub priority: u8,
    /// Account identifier (for multi-account support)
    pub account_id: u32,
}

/// Inbox statistics for a folder
#[derive(Clone, Copy, Debug)]
pub struct FolderStats {
    /// Folder identifier
    pub folder: Folder,
    /// Total message count
    pub total: u32,
    /// Unread message count
    pub unread: u32,
    /// Flagged message count
    pub flagged: u32,
    /// Total size in bytes
    pub total_size: u64,
}

/// Paginated result set
#[derive(Clone, Debug)]
pub struct MessagePage {
    /// Messages in this page
    pub messages: Vec<EmailMessage>,
    /// Total number of matching messages
    pub total: usize,
    /// Current page number (0-based)
    pub page: usize,
    /// Page size
    pub page_size: usize,
    /// Whether there are more pages
    pub has_more: bool,
}

// ── Global State ───────────────────────────────────────────────────────

static INBOX: Mutex<Option<Vec<EmailMessage>>> = Mutex::new(None);
static CUSTOM_FOLDERS: Mutex<Option<Vec<u64>>> = Mutex::new(None);
static NEXT_MSG_ID: Mutex<u64> = Mutex::new(1);

// ── Helper Functions ───────────────────────────────────────────────────

/// Generate the next unique message ID
fn alloc_msg_id() -> u64 {
    let mut id = NEXT_MSG_ID.lock();
    let current = *id;
    *id = current.wrapping_add(1);
    current
}

/// Check if a message matches a folder
fn in_folder(msg: &EmailMessage, folder: Folder) -> bool {
    match folder {
        Folder::AllMail => msg.folder != Folder::Trash && msg.folder != Folder::Spam,
        Folder::Starred => msg.flagged,
        _ => msg.folder == folder,
    }
}

/// Compare two messages according to the sort order
fn compare_messages(a: &EmailMessage, b: &EmailMessage, sort: SortBy) -> core::cmp::Ordering {
    match sort {
        SortBy::DateDesc => b.date.cmp(&a.date),
        SortBy::DateAsc => a.date.cmp(&b.date),
        SortBy::SenderAsc => a.from.cmp(&b.from),
        SortBy::SenderDesc => b.from.cmp(&a.from),
        SortBy::SubjectAsc => a.subject_hash.cmp(&b.subject_hash),
        SortBy::SubjectDesc => b.subject_hash.cmp(&a.subject_hash),
        SortBy::SizeDesc => b.size.cmp(&a.size),
        SortBy::SizeAsc => a.size.cmp(&b.size),
        SortBy::UnreadFirst => {
            if a.read != b.read {
                if a.read {
                    core::cmp::Ordering::Greater
                } else {
                    core::cmp::Ordering::Less
                }
            } else {
                b.date.cmp(&a.date)
            }
        }
        SortBy::FlaggedFirst => {
            if a.flagged != b.flagged {
                if a.flagged {
                    core::cmp::Ordering::Less
                } else {
                    core::cmp::Ordering::Greater
                }
            } else {
                b.date.cmp(&a.date)
            }
        }
    }
}

/// Simple insertion sort for message lists (no std sort available)
fn sort_messages(messages: &mut Vec<EmailMessage>, sort: SortBy) {
    let len = messages.len();
    for i in 1..len {
        let mut j = i;
        while j > 0
            && compare_messages(&messages[j - 1], &messages[j], sort)
                == core::cmp::Ordering::Greater
        {
            messages.swap(j - 1, j);
            j -= 1;
        }
    }
}

// ── Core Inbox Operations ─────────────────────────────────────────────

/// Store a new message in the inbox
pub fn store_message(msg: EmailMessage) -> u64 {
    let mut inbox = INBOX.lock();
    if let Some(ref mut list) = *inbox {
        if list.len() >= MAX_MESSAGES {
            // Evict the oldest trash message, or oldest overall
            let trash_pos = list.iter().position(|m| m.folder == Folder::Trash);
            if let Some(pos) = trash_pos {
                list.remove(pos);
            } else {
                list.remove(0);
            }
        }
        let id = msg.id;
        list.push(msg);
        serial_println!("[inbox] Stored message id={}", id);
        id
    } else {
        0
    }
}

/// Create and store a new message with basic fields
pub fn add_message(
    from: u64,
    to: u64,
    subject_hash: u64,
    body_hash: u64,
    date: u64,
    folder: Folder,
) -> u64 {
    let id = alloc_msg_id();
    let msg = EmailMessage {
        id,
        from,
        to,
        subject_hash,
        body_hash,
        date,
        read: false,
        flagged: false,
        folder,
        has_attachments: false,
        size: 0,
        thread_id: subject_hash, // thread by subject
        reply_to_id: 0,
        cc_count: 0,
        answered: false,
        is_draft: folder == Folder::Drafts,
        priority: 3,
        account_id: 0,
    };
    store_message(msg)
}

/// Get messages in a specific folder, paginated and sorted
pub fn get_messages(folder: Folder, sort: SortBy, page: usize, page_size: usize) -> MessagePage {
    let actual_page_size = if page_size == 0 {
        DEFAULT_PAGE_SIZE
    } else {
        page_size
    };

    let inbox = INBOX.lock();
    if let Some(ref list) = *inbox {
        let mut matching: Vec<EmailMessage> = list
            .iter()
            .filter(|m| in_folder(m, folder))
            .copied()
            .collect();

        let total = matching.len();
        sort_messages(&mut matching, sort);

        let start = page * actual_page_size;
        let end = if start + actual_page_size > total {
            total
        } else {
            start + actual_page_size
        };

        let page_messages = if start < total {
            matching[start..end].to_vec()
        } else {
            Vec::new()
        };

        let has_more = end < total;

        MessagePage {
            messages: page_messages,
            total,
            page,
            page_size: actual_page_size,
            has_more,
        }
    } else {
        MessagePage {
            messages: Vec::new(),
            total: 0,
            page: 0,
            page_size: actual_page_size,
            has_more: false,
        }
    }
}

/// Move a message to a different folder
pub fn move_to_folder(msg_id: u64, target: Folder) -> bool {
    let mut inbox = INBOX.lock();
    if let Some(ref mut list) = *inbox {
        if let Some(msg) = list.iter_mut().find(|m| m.id == msg_id) {
            let old_folder = msg.folder;
            msg.folder = target;
            msg.is_draft = target == Folder::Drafts;
            serial_println!(
                "[inbox] Moved message {} from {:?} to {:?}",
                msg_id,
                old_folder,
                target
            );
            return true;
        }
    }
    false
}

/// Toggle or set the flagged/starred state
pub fn flag(msg_id: u64, flagged: bool) -> bool {
    let mut inbox = INBOX.lock();
    if let Some(ref mut list) = *inbox {
        if let Some(msg) = list.iter_mut().find(|m| m.id == msg_id) {
            msg.flagged = flagged;
            return true;
        }
    }
    false
}

/// Mark a message as read or unread
pub fn mark_read(msg_id: u64, read: bool) -> bool {
    let mut inbox = INBOX.lock();
    if let Some(ref mut list) = *inbox {
        if let Some(msg) = list.iter_mut().find(|m| m.id == msg_id) {
            msg.read = read;
            return true;
        }
    }
    false
}

/// Mark all messages in a folder as read
pub fn mark_all_read(folder: Folder) -> u32 {
    let mut count: u32 = 0;
    let mut inbox = INBOX.lock();
    if let Some(ref mut list) = *inbox {
        for msg in list.iter_mut() {
            if in_folder(msg, folder) && !msg.read {
                msg.read = true;
                count = count.saturating_add(1);
            }
        }
    }
    serial_println!("[inbox] Marked {} messages as read in {:?}", count, folder);
    count
}

/// Delete a message (move to Trash, or permanently remove if already in Trash)
pub fn delete(msg_id: u64) -> bool {
    let mut inbox = INBOX.lock();
    if let Some(ref mut list) = *inbox {
        if let Some(pos) = list.iter().position(|m| m.id == msg_id) {
            if list[pos].folder == Folder::Trash {
                // Permanent delete
                list.remove(pos);
                serial_println!("[inbox] Permanently deleted message {}", msg_id);
            } else {
                // Move to trash
                list[pos].folder = Folder::Trash;
                serial_println!("[inbox] Moved message {} to Trash", msg_id);
            }
            return true;
        }
    }
    false
}

/// Empty the Trash folder (permanently delete all messages in Trash)
pub fn empty_trash() -> u32 {
    let mut count: u32 = 0;
    let mut inbox = INBOX.lock();
    if let Some(ref mut list) = *inbox {
        let before = list.len();
        list.retain(|m| m.folder != Folder::Trash);
        count = (before - list.len()) as u32;
    }
    serial_println!("[inbox] Emptied trash ({} messages removed)", count);
    count
}

/// Search messages matching a query hash in the specified field
pub fn search(query_hash: u64, field: SearchField, folder: Option<Folder>) -> Vec<EmailMessage> {
    let inbox = INBOX.lock();
    let mut results = Vec::new();

    if let Some(ref list) = *inbox {
        for msg in list.iter() {
            // If folder filter is specified, apply it
            if let Some(f) = folder {
                if !in_folder(msg, f) {
                    continue;
                }
            }

            let matched = match field {
                SearchField::All => {
                    msg.from == query_hash
                        || msg.to == query_hash
                        || msg.subject_hash == query_hash
                        || msg.body_hash == query_hash
                }
                SearchField::From => msg.from == query_hash,
                SearchField::To => msg.to == query_hash,
                SearchField::Subject => msg.subject_hash == query_hash,
                SearchField::Body => msg.body_hash == query_hash,
                SearchField::HasAttachment => msg.has_attachments,
                SearchField::DateRange => {
                    // Interpret query_hash as a date range: high 32 bits = start, low 32 bits = end
                    let start = (query_hash >> 32) as u64;
                    let end = (query_hash & 0xFFFFFFFF) as u64;
                    msg.date >= start && msg.date <= end
                }
            };

            if matched {
                results.push(*msg);
                if results.len() >= MAX_SEARCH_RESULTS {
                    break;
                }
            }
        }
    }

    serial_println!("[inbox] Search returned {} results", results.len());
    results
}

/// Get unread count for a specific folder
pub fn get_unread_count(folder: Folder) -> u32 {
    let inbox = INBOX.lock();
    if let Some(ref list) = *inbox {
        list.iter()
            .filter(|m| in_folder(m, folder) && !m.read)
            .count() as u32
    } else {
        0
    }
}

/// Get unread counts for all standard folders
pub fn get_all_unread_counts() -> Vec<(Folder, u32)> {
    let folders = [
        Folder::Inbox,
        Folder::Sent,
        Folder::Drafts,
        Folder::Trash,
        Folder::Spam,
        Folder::Archive,
    ];
    let mut counts = Vec::new();
    for &folder in &folders {
        counts.push((folder, get_unread_count(folder)));
    }
    counts
}

/// Get statistics for a folder
pub fn get_folder_stats(folder: Folder) -> FolderStats {
    let inbox = INBOX.lock();
    let mut stats = FolderStats {
        folder,
        total: 0,
        unread: 0,
        flagged: 0,
        total_size: 0,
    };

    if let Some(ref list) = *inbox {
        for msg in list.iter() {
            if in_folder(msg, folder) {
                stats.total = stats.total.saturating_add(1);
                if !msg.read {
                    stats.unread = stats.unread.saturating_add(1);
                }
                if msg.flagged {
                    stats.flagged = stats.flagged.saturating_add(1);
                }
                stats.total_size = stats.total_size.saturating_add(msg.size as u64);
            }
        }
    }

    stats
}

/// Get a single message by ID
pub fn get_message(msg_id: u64) -> Option<EmailMessage> {
    let inbox = INBOX.lock();
    if let Some(ref list) = *inbox {
        list.iter().find(|m| m.id == msg_id).copied()
    } else {
        None
    }
}

/// Get all messages in a thread
pub fn get_thread(thread_id: u64) -> Vec<EmailMessage> {
    let inbox = INBOX.lock();
    if let Some(ref list) = *inbox {
        let mut thread: Vec<EmailMessage> = list
            .iter()
            .filter(|m| m.thread_id == thread_id)
            .copied()
            .collect();
        sort_messages(&mut thread, SortBy::DateAsc);
        thread
    } else {
        Vec::new()
    }
}

/// Create a custom folder
pub fn create_custom_folder(name_hash: u64) -> bool {
    let mut folders = CUSTOM_FOLDERS.lock();
    if let Some(ref mut list) = *folders {
        if list.len() >= MAX_CUSTOM_FOLDERS {
            return false;
        }
        if list.contains(&name_hash) {
            return false; // already exists
        }
        list.push(name_hash);
        serial_println!("[inbox] Created custom folder 0x{:016X}", name_hash);
        true
    } else {
        false
    }
}

/// Delete a custom folder (moves messages to Inbox)
pub fn delete_custom_folder(name_hash: u64) -> bool {
    let mut folders = CUSTOM_FOLDERS.lock();
    if let Some(ref mut list) = *folders {
        if let Some(pos) = list.iter().position(|&h| h == name_hash) {
            list.remove(pos);

            // Move messages from this folder to Inbox
            let mut inbox = INBOX.lock();
            if let Some(ref mut msgs) = *inbox {
                for msg in msgs.iter_mut() {
                    if msg.folder == Folder::Custom(name_hash) {
                        msg.folder = Folder::Inbox;
                    }
                }
            }

            serial_println!("[inbox] Deleted custom folder 0x{:016X}", name_hash);
            return true;
        }
    }
    false
}

/// List all custom folder hashes
pub fn list_custom_folders() -> Vec<u64> {
    let folders = CUSTOM_FOLDERS.lock();
    if let Some(ref list) = *folders {
        list.clone()
    } else {
        Vec::new()
    }
}

/// Get total message count across all folders
pub fn total_count() -> usize {
    let inbox = INBOX.lock();
    if let Some(ref list) = *inbox {
        list.len()
    } else {
        0
    }
}

/// Sort messages in-place by the given criteria and return them
pub fn sort_by(messages: &mut Vec<EmailMessage>, sort: SortBy) {
    sort_messages(messages, sort);
}

/// Initialize the inbox manager subsystem
pub fn init() {
    {
        let mut inbox = INBOX.lock();
        *inbox = Some(Vec::new());
    }
    {
        let mut folders = CUSTOM_FOLDERS.lock();
        *folders = Some(Vec::new());
    }
    serial_println!(
        "[inbox] Inbox manager initialized (capacity: {} messages)",
        MAX_MESSAGES
    );
}
