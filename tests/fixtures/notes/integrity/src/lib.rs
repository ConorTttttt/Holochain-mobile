//! A deliberately restrictive fixture: an accept-all DNA cannot show that
//! rejections survive the H2HC adapter.
//!
//! - A note must be non-empty and at most `MAX_NOTE_CHARS` characters.
//! - Only a note's author may update or delete it.
//! - An `AuthorToNote` link must start at its creator's own agent key, and
//!   only its creator may delete it.

use hdi::prelude::*;

pub const MAX_NOTE_CHARS: usize = 280;

#[hdk_entry_helper]
#[derive(Clone, PartialEq)]
pub struct Note {
    pub text: String,
}

#[hdk_entry_types]
#[unit_enum(UnitEntryTypes)]
pub enum EntryTypes {
    Note(Note),
}

#[hdk_link_types]
pub enum LinkTypes {
    AuthorToNote,
}

fn validate_author_link(action: &TypedAction<CreateLinkData>) -> ValidateCallbackResult {
    if action.data.base_address == AnyLinkableHash::from(action.author().clone()) {
        ValidateCallbackResult::Valid
    } else {
        ValidateCallbackResult::Invalid("an author can only link from their own key".into())
    }
}

fn validate_note(note: &Note) -> ValidateCallbackResult {
    if note.text.trim().is_empty() {
        ValidateCallbackResult::Invalid("a note cannot be empty".into())
    } else if note.text.chars().count() > MAX_NOTE_CHARS {
        ValidateCallbackResult::Invalid(format!(
            "a note is at most {MAX_NOTE_CHARS} characters"
        ))
    } else {
        ValidateCallbackResult::Valid
    }
}

fn by_original_author(
    original: ActionHash,
    author: &AgentPubKey,
) -> ExternResult<ValidateCallbackResult> {
    let original = must_get_action(original)?;
    if original.action().author() == author {
        Ok(ValidateCallbackResult::Valid)
    } else {
        Ok(ValidateCallbackResult::Invalid(
            "only a note's author may change it".into(),
        ))
    }
}

fn validate_update(
    note: &Note,
    action: &TypedAction<UpdateData>,
) -> ExternResult<ValidateCallbackResult> {
    match validate_note(note) {
        ValidateCallbackResult::Valid => {
            by_original_author(action.data.original_action_address.clone(), action.author())
        }
        invalid => Ok(invalid),
    }
}

#[hdk_extern]
pub fn validate(op: Op) -> ExternResult<ValidateCallbackResult> {
    match op.flattened::<EntryTypes, LinkTypes>()? {
        FlatOp::CreateEntry(OpEntry::CreateEntry {
            app_entry: EntryTypes::Note(note),
            ..
        })
        | FlatOp::CreateRecord(OpRecord::CreateEntry {
            app_entry: EntryTypes::Note(note),
            ..
        }) => Ok(validate_note(&note)),
        FlatOp::CreateEntry(OpEntry::UpdateEntry {
            app_entry: EntryTypes::Note(note),
            action,
        })
        | FlatOp::Update(OpUpdate::Entry {
            app_entry: EntryTypes::Note(note),
            action,
        })
        | FlatOp::CreateRecord(OpRecord::UpdateEntry {
            app_entry: EntryTypes::Note(note),
            action,
        }) => validate_update(&note, &action),
        FlatOp::Delete(OpDelete { action })
        | FlatOp::CreateRecord(OpRecord::DeleteEntry { action }) => {
            by_original_author(action.data.deletes_address.clone(), action.author())
        }
        FlatOp::Link(OpLink::CreateLink {
            link_type: LinkTypes::AuthorToNote,
            action,
        })
        | FlatOp::CreateRecord(OpRecord::CreateLink {
            link_type: LinkTypes::AuthorToNote,
            action,
        }) => Ok(validate_author_link(&action)),
        FlatOp::Link(OpLink::DeleteLink {
            original_action,
            action,
            ..
        }) => Ok(if original_action.author() == action.author() {
            ValidateCallbackResult::Valid
        } else {
            ValidateCallbackResult::Invalid("only a link's creator may delete it".into())
        }),
        _ => Ok(ValidateCallbackResult::Valid),
    }
}
