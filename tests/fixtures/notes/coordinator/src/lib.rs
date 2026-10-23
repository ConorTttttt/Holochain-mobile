use hdk::prelude::*;
use notes_integrity::{EntryTypes, Note};

#[hdk_extern]
pub fn create_note(text: String) -> ExternResult<ActionHash> {
    create_entry(&EntryTypes::Note(Note { text }))
}

#[hdk_extern]
pub fn get_note(hash: ActionHash) -> ExternResult<Option<Record>> {
    get(hash, GetOptions::network())
}
