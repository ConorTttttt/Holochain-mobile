use hdk::prelude::*;
use notes_integrity::{EntryTypes, LinkTypes, Note};

/// Signal payload for measuring signal round trips.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Ping {
    Ping { id: u64 },
    Pong { id: u64 },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct EchoInput {
    pub agent: AgentPubKey,
    pub text: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PingInput {
    pub agent: AgentPubKey,
    pub id: u64,
}

/// Let any agent call `echo` and deliver remote signals.
#[hdk_extern]
pub fn init() -> ExternResult<InitCallbackResult> {
    let zome = zome_info()?.name;
    let mut functions = HashSet::new();
    functions.insert((zome.clone(), FunctionName::from("echo")));
    functions.insert((zome, FunctionName::from("recv_remote_signal")));
    create_cap_grant(CapGrant::new_zome_call_grant(
        "remote".into(),
        GrantConstraint::Unrestricted,
        GrantedFunctions::Listed(functions),
    ))?;
    Ok(InitCallbackResult::Pass)
}

#[hdk_extern]
pub fn create_note(text: String) -> ExternResult<ActionHash> {
    let hash = create_entry(&EntryTypes::Note(Note { text }))?;
    let me = agent_info()?.agent_initial_pubkey;
    create_link(me, hash.clone(), LinkTypes::AuthorToNote, ())?;
    Ok(hash)
}

#[hdk_extern]
pub fn get_note(hash: ActionHash) -> ExternResult<Option<Record>> {
    get(hash, GetOptions::network())
}

#[hdk_extern]
pub fn get_notes(author: AgentPubKey) -> ExternResult<Vec<Link>> {
    get_links(
        LinkQuery::try_new(author, LinkTypes::AuthorToNote)?,
        GetStrategy::Network,
    )
}

#[hdk_extern]
pub fn echo(text: String) -> ExternResult<String> {
    Ok(text)
}

#[hdk_extern]
pub fn call_echo(input: EchoInput) -> ExternResult<String> {
    match call_remote(input.agent, zome_info()?.name, "echo".into(), None, input.text)? {
        ZomeCallResponse::Ok(io) => io.decode().map_err(|e| wasm_error!(e)),
        other => Err(wasm_error!(WasmErrorInner::Guest(format!("{other:?}")))),
    }
}

#[hdk_extern]
pub fn ping(input: PingInput) -> ExternResult<()> {
    send_remote_signal(Ping::Ping { id: input.id }, vec![input.agent])
}

/// Answer pings with a pong, and pass both to the local client.
#[hdk_extern]
pub fn recv_remote_signal(signal: ExternIO) -> ExternResult<()> {
    let msg: Ping = signal.decode().map_err(|e| wasm_error!(e))?;
    if let Ping::Ping { id } = msg {
        send_remote_signal(Ping::Pong { id }, vec![call_info()?.provenance])?;
    }
    emit_signal(msg)
}
