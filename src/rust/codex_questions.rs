//! Phone-owned Windows CLI question sharing. Bearers never leave this backend.
use serde_json::{json, Value};
use crate::conversation::{ConversationManager, NodeMetadata, NodeType};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use tokio::sync::{broadcast, mpsc, Notify};
const READY_ID: &str = "iterate-codex-native-ready-v1";
const OPEN: &str = "<send_user_message_question_reply>";
const CLOSE: &str = "</send_user_message_question_reply>";
const MAX_GROUPS: usize = 256;
const MAX_QUESTIONS: usize = 1024;
const MAX_IDENTITIES: usize = 4096;
const MAX_ACTIONS: usize = 8192;
const MAX_SENDS: usize = 32;
const MAX_ANSWER: usize = 32 * 1024;
static STARTED: AtomicBool = AtomicBool::new(false);
static HUB: once_cell::sync::Lazy<Mutex<Hub>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Hub::default()));
static EVENTS: once_cell::sync::Lazy<broadcast::Sender<Value>> =
    once_cell::sync::Lazy::new(|| broadcast::channel(2048).0);
static CHANGED: Notify = Notify::const_new();
static HISTORY_SERIAL: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));
static HISTORY_WAITING: once_cell::sync::Lazy<Mutex<HashSet<String>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashSet::new()));
static HISTORY_READY: Notify = Notify::const_new();
type ActionKey = (String, String);
#[derive(Default)]
struct Hub {
    owner: String,
    questions: Vec<Question>,
    issued: HashMap<String, Identity>,
    staged: HashMap<String, Stage>,
    group_devices: HashMap<String, String>,
    actions: HashMap<ActionKey, ActionEntry>,
    sessions: HashMap<String, mpsc::Sender<Submission>>,
    published: HashMap<String, Value>,
    pending_sends: usize,
}
#[derive(Clone)]
struct Identity {
    owner: String,
    launch: String,
    thread: String,
    turn: String,
    call: String,
    index: usize,
    cwd: String,
}
#[derive(Clone)]
struct Question {
    id: String,
    owner: String,
    launch: String,
    thread: String,
    turn: String,
    call: String,
    index: usize,
    title: String,
    conversation_title: Option<String>,
    thread_preview: Option<String>,
    options: Vec<String>,
    cwd: String,
    hidden: bool,
    status: &'static str,
}
impl Question {
    fn timeline_route(&self) -> String {
        format!("native-cli:{}:{}", self.launch, self.thread)
    }
    fn group(&self) -> Value {
        json!([self.owner, self.launch, self.thread, self.turn, self.call])
    }
    fn item_id(&self) -> String {
        json!(["request_user_input_async", self.call, self.index]).to_string()
    }
    fn identity(&self) -> Identity {
        Identity {
            owner: self.owner.clone(),
            launch: self.launch.clone(),
            thread: self.thread.clone(),
            turn: self.turn.clone(),
            call: self.call.clone(),
            index: self.index,
            cwd: self.cwd.clone(),
        }
    }
    fn public(&self) -> Value {
        json!({"id":self.id,"source":"codex_native","message":self.title,"predefined_options":self.options,"project_path":self.cwd,"codex_thread_id":self.thread,"timeline_route_id":self.timeline_route(),"conversation_title":native_conversation_title(self.conversation_title.as_deref(),self.thread_preview.as_deref(),&self.thread),"is_markdown":false,
        "native":{"owner":self.owner,"group_id":self.group().to_string(),"index":self.index,"question_item_id":self.item_id(),"status":self.status}})
    }
}

fn normalized_thread_name(value: &Value) -> Option<String> {
    value.as_str().map(str::trim).filter(|name| !name.is_empty()).map(str::to_owned)
}

fn resumed_thread_name(thread: &Value, expected_thread_id: &str) -> Option<String> {
    if thread.get("id").and_then(Value::as_str) != Some(expected_thread_id) { return None; }
    normalized_thread_name(&thread["name"])
}

fn resumed_thread_preview(thread: &Value, expected_thread_id: &str) -> Option<String> {
    if thread.get("id").and_then(Value::as_str) != Some(expected_thread_id) { return None; }
    normalized_thread_name(&thread["preview"])
}

fn native_conversation_title(name: Option<&str>, preview: Option<&str>, thread_id: &str) -> String {
    if let Some(name)=name.map(str::trim).filter(|name| !name.is_empty()) { return name.to_owned(); }
    // This suffix is display-only. Exact thread/request identities remain untouched.
    let short_id=thread_id.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<String>();
    let short_id=if short_id.is_empty() { "未知ID" } else { &short_id };
    let paragraph=preview.unwrap_or_default().trim().lines().next().unwrap_or_default().trim();
    if paragraph.is_empty() { return format!("未命名会话 · {short_id}"); }
    let sentence_end=paragraph.char_indices().find(|(offset,c)| match c {
        '。'|'！'|'？'=>true,
        '.'|'!'|'?'=>paragraph[*offset+c.len_utf8()..].chars().next().map_or(true,char::is_whitespace),
        _=>false,
    })
        .map(|(offset,c)|offset+c.len_utf8()).unwrap_or(paragraph.len());
    let sentence=paragraph[..sentence_end].trim();
    let mut prefix=sentence.chars().take(64).collect::<String>();
    if sentence.chars().nth(64).is_some() { prefix.push('…'); }
    format!("{prefix} · {short_id}")
}

fn rename_pending_question_titles(h: &mut Hub, owner: &str, launch: &str, thread: &str,
    name: Option<String>) -> bool {
    if h.owner != owner { return false; }
    let mut changed = false;
    for question in &mut h.questions {
        if question.owner == owner && question.launch == launch && question.thread == thread
            && question.conversation_title != name {
            question.conversation_title = name.clone();
            changed = true;
        }
    }
    changed
}

async fn record_native_history(q: &Question, kind: NodeType, content: String) {
    let route = q.timeline_route();
    while HISTORY_WAITING.lock().unwrap_or_else(|e|e.into_inner()).contains(&route) {
        if !current_owner(&q.owner) { return; }
        let ready = HISTORY_READY.notified();
        tokio::pin!(ready);
        ready.as_mut().enable();
        if !HISTORY_WAITING.lock().unwrap_or_else(|e|e.into_inner()).contains(&route) { break; }
        let _ = tokio::time::timeout(std::time::Duration::from_secs(20), ready).await;
    }
    let event_id = native_question_history_event_id(q, &kind);
    record_native_history_event(q, kind, content, event_id).await;
}

fn native_question_history_event_id(q: &Question, kind: &NodeType) -> String {
    // The Bridge owner changes after restart; the actual CLI question identity does not.
    format!("native-event:{}:{}", kind.as_key(), json!([q.launch, q.thread, q.turn, q.call, q.index]))
}

fn release_initial_history(route: &str) {
    HISTORY_WAITING.lock().unwrap_or_else(|e|e.into_inner()).remove(route);
    HISTORY_READY.notify_waiters();
}

async fn record_native_history_event(q: &Question, kind: NodeType, content: String, event_id: String) {
    if content.trim().is_empty() || !current_owner(&q.owner) { return; }
    let _guard = HISTORY_SERIAL.lock().await;
    if !current_owner(&q.owner) { return; }
    let manager = ConversationManager::new_with_forced_persistence();
    let route = q.timeline_route();
    // A Native tree is bound only to its launch/thread route, never to cwd's project map.
    let tree = manager.get_or_create_tree_for_route(Some(&route), None).await;
    let metadata = NodeMetadata {
        conversation_id: Some(tree.clone()),
        project_path: Some(q.cwd.clone()),
        request_id: Some(route.clone()),
        run_id: Some(event_id),
        source: Some("codex_native".to_string()),
        ..NodeMetadata::default()
    };
    match manager.add_node_for_event(&tree, kind, content, metadata).await {
        Ok(outcome) if !outcome.reused => {
            if let Some(node) = manager.get_node(&tree, &outcome.node_id).await {
                let _ = EVENTS.send(json!({"message_type":"timeline_sync_delta","native_owner":q.owner,
                    "payload":{"source":"codex_native","request_id":route,"timeline_route_id":route,
                    "project_path":q.cwd,"conversation_id":tree,"timelineNode":node}}));
                let sessions = manager.list_native_history_sessions().await;
                let _ = EVENTS.send(json!({"message_type":"native_history_sessions","native_owner":q.owner,
                    "payload":{"source":"codex_native","sessions":sessions}}));
            }
        }
        Err(error) => log::warn!("[CLI questions] history write failed: {}", error),
        _ => {}
    }
}

pub(crate) async fn history_snapshot(route: &str, path: &str) -> Option<Value> {
    if !route.starts_with("native-cli:") || path.trim().is_empty() { return None; }
    let _guard = HISTORY_SERIAL.lock().await;
    let manager = ConversationManager::new_with_forced_persistence();
    let tree = manager.get_tree_for_route(Some(route), None).await?;
    let node = manager.get_current_node_id(&tree).await?;
    let nodes = manager.get_node_path(&tree, &node).await.ok()?;
    // Exact route and path check prevents a caller from crossing CLI instances in one cwd.
    if nodes.iter().any(|node| node.metadata.request_id.as_deref() != Some(route)
        || node.metadata.project_path.as_deref() != Some(path)
        || node.metadata.source.as_deref() != Some("codex_native")) { return None; }
    Some(json!({"message_type":"timeline_sync_snapshot","payload":{"source":"codex_native",
        "request_id":route,"timeline_route_id":route,"project_path":path,
        "conversation_id":tree,"timelineNodes":nodes}}))
}

pub(crate) async fn history_sessions() -> Value {
    let _guard = HISTORY_SERIAL.lock().await;
    let manager = ConversationManager::new_with_forced_persistence();
    json!({"message_type":"native_history_sessions","payload":{"source":"codex_native",
        "sessions":manager.list_native_history_sessions().await}})
}

pub(crate) async fn attach_history_to_state(event: &mut Value) {
    if event["message_type"] != "mcp_state" { return; }
    let request = &event["payload"]["request"];
    let (Some(route), Some(path)) = (request["timeline_route_id"].as_str().map(str::to_owned), request["project_path"].as_str().map(str::to_owned)) else { return; };
    if let Some(history) = history_snapshot(&route, &path).await {
        event["payload"]["timeline_route_id"] = json!(route);
        event["payload"]["project_path"] = json!(path);
        event["payload"]["conversation_id"] = history["payload"]["conversation_id"].clone();
        event["payload"]["timelineNodes"] = history["payload"]["timelineNodes"].clone();
    }
}
#[derive(Clone)]
struct Stage {
    answer: String,
    saved: bool,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ActionState {
    Pending,
    Unknown,
    Saved,
    CliReceived,
    Rejected,
}
struct ActionEntry {
    payload: Value,
    ids: Vec<String>,
    state: ActionState,
    conflict: bool,
    inflight: bool,
    reason: Option<String>,
}
struct Submission {
    owner: String,
    question: Question,
    answers: HashMap<String, String>,
    action_key: ActionKey,
}
fn lock_hub() -> std::sync::MutexGuard<'static, Hub> {
    HUB.lock().unwrap_or_else(|e| e.into_inner())
}
fn current_owner(owner: &str) -> bool {
    !owner.is_empty() && lock_hub().owner == owner
}
fn state_message(q: Value) -> Value {
    json!({"message_type":"mcp_state","payload":{"source":"codex_native","request":q,"showMcpPopup":true}})
}
fn closed_message(id: &str, path: &str) -> Value {
    json!({"message_type":"mcp_state","payload":{"source":"codex_native","request":null,"showMcpPopup":false,"request_id":id,"project_path":path}})
}
fn visible(h: &Hub) -> HashMap<String, Value> {
    h.questions
        .iter()
        .filter(|q| !q.hidden && !h.staged.get(&q.id).is_some_and(|a| a.saved))
        .map(|q| (q.id.clone(), q.public()))
        .collect()
}
fn publish(owner: &str) {
    let events = {
        let mut h = lock_hub();
        if h.owner != owner {
            return;
        }
        let next = visible(&h);
        // Record the owner's actual visibility transition before publishing it.
        // A missing snapshot after restart is not evidence of completion.
        if crate::cross_device::hub_transport::config().ok().flatten().is_some() {
            if let Err(error)=persist_native_visibility(&h, &next) {
                log::warn!("native visibility persistence failed: {}",error);
                return;
            }
        }
        let mut events = Vec::new();
        for (id, q) in &h.published {
            if !next.contains_key(id) {
                events.push(closed_message(
                    id,
                    q["project_path"].as_str().unwrap_or_default(),
                ));
            }
        }
        for q in &h.questions {
            if let Some(value) = next.get(&q.id) {
                if h.published.get(&q.id) != Some(value) {
                    events.push(state_message(value.clone()));
                }
            }
        }
        h.published = next;
        events
    };
    for event in events {
        let _ = EVENTS.send(event);
    }
    CHANGED.notify_waiters();
}
pub(crate) fn subscribe() -> broadcast::Receiver<Value> {
    EVENTS.subscribe()
}
pub(crate) fn snapshot() -> Vec<Value> {
    let h = lock_hub();
    h.questions
        .iter()
        .filter(|q| !q.hidden && !h.staged.get(&q.id).is_some_and(|a| a.saved))
        .map(|q| state_message(q.public()))
        .collect()
}
pub(crate) fn event_is_current(event:&Value)->bool {
    let h=lock_hub();
    if event["message_type"] == "timeline_sync_delta" || event["message_type"] == "native_history_sessions" {
        return event["native_owner"].as_str() == Some(h.owner.as_str());
    }
    if event["message_type"]=="mcp_state" {
        let q=&event["payload"]["request"];if q.is_null(){return true;}
        return q["id"].as_str().is_some_and(|id|h.published.get(id)==Some(q));
    }
    if event["message_type"]=="mcp_action_result"{
        let Some(device)=event["native_device"].as_str()else{return false;};
        let Some(action)=event["payload"]["client_action_id"].as_str()else{return false;};
        let mut value=event.clone();value.as_object_mut().unwrap().remove("native_device");
        return ledger_result(&h,&(device.to_owned(),action.to_owned())).is_some_and(|v|v==value);
    }
    false
}
// Retain the registered GUI commands for old callers, without starting or emitting a GUI.
pub fn ready(id: Option<&str>) -> bool {
    id == Some(READY_ID)
}
pub fn registered(id: &str) -> bool {
    lock_hub().issued.contains_key(id)
}
pub async fn respond(_id: &str, _response: &Value) -> Result<(), String> {
    Err("CLI 提问请在已配对的手机上回答".into())
}
pub(crate) fn is_native_action(payload: &Value) -> bool {
    payload
        .get("request_id")
        .or_else(|| payload.get("requestId"))
        .and_then(Value::as_str)
        .is_some_and(|id| id.starts_with("native-codex:"))
        || payload.get("source").and_then(Value::as_str) == Some("codex_native")
        || payload.get("native_query").is_some()
}
fn canonical(payload: &Value) -> Value {
    let mut value = payload.clone();
    if let Some(o) = value.as_object_mut() {
        o.remove("native_query");
    }
    value
}
// All native entry points retain the original mutex. Disk stores only the
// canonical digest, identifiers, receipt and an existing history-node reference.
fn native_ledger() -> Result<rusqlite::Connection, String> {
    let dir = crate::cross_device::hub_transport::data_directory()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let db = rusqlite::Connection::open(dir.join("native-actions.sqlite")).map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_secs(10)).map_err(|e| e.to_string())?;
    db.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS actions(device TEXT NOT NULL,action TEXT NOT NULL,digest TEXT NOT NULL,receipt TEXT NOT NULL,history_ref TEXT,PRIMARY KEY(device,action)); CREATE TABLE IF NOT EXISTS visibility(request_id TEXT PRIMARY KEY,closed INTEGER NOT NULL);").map_err(|e| e.to_string())?;
    Ok(db)
}
fn persist_native_visibility(h:&Hub,next:&HashMap<String,Value>)->Result<(),String> {
    if h.published==*next {return Ok(());}
    let mut db=native_ledger()?;
    let tx=db.transaction().map_err(|e|e.to_string())?;
    for id in h.published.keys().filter(|id|!next.contains_key(*id)) {
        tx.execute("INSERT INTO visibility(request_id,closed) VALUES(?1,1) ON CONFLICT(request_id) DO UPDATE SET closed=1",[id]).map_err(|e|e.to_string())?;
    }
    for id in next.keys().filter(|id|!h.published.contains_key(*id)) {
        tx.execute("INSERT INTO visibility(request_id,closed) VALUES(?1,0) ON CONFLICT(request_id) DO UPDATE SET closed=0",[id]).map_err(|e|e.to_string())?;
    }
    tx.commit().map_err(|e|e.to_string())
}
pub(crate) fn hub_native_closed(id:&str)->Result<bool,String> {
    use rusqlite::OptionalExtension;
    let h=lock_hub();
    if !h.owner.is_empty() {
        // Retry a failed owner transition under its original lock, even when no
        // further CLI event arrives to trigger publish().
        persist_native_visibility(&h,&visible(&h))?;
    }
    Ok(native_ledger()?.query_row("SELECT closed FROM visibility WHERE request_id=?1",[id],|r|r.get::<_,bool>(0))
        .optional().map_err(|e|e.to_string())?.unwrap_or(false))
}
pub(crate) fn hub_native_quiescent() -> bool { lock_hub().pending_sends == 0 }
fn native_digest(payload: &Value) -> String {
    hex::encode(ring::digest::digest(&ring::digest::SHA256, canonical(payload).to_string().as_bytes()))
}
fn persist_native(h: &Hub, key: &ActionKey, history_ref: Option<&Value>) -> Result<(), String> {
    let entry = h.actions.get(key).ok_or("native_action_missing")?;
    let receipt = ledger_result(h, key).ok_or("native_action_missing")?;
    let db = native_ledger()?;
    let changed = db.execute("INSERT INTO actions(device,action,digest,receipt,history_ref) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(device,action) DO UPDATE SET receipt=excluded.receipt,history_ref=COALESCE(excluded.history_ref,actions.history_ref) WHERE actions.digest=excluded.digest",
        rusqlite::params![key.0,key.1,native_digest(&entry.payload),receipt.to_string(),history_ref.map(Value::to_string)]).map_err(|e| e.to_string())?;
    if changed != 1 { return Err("action_id_content_mismatch".into()); }
    Ok(())
}
pub(crate) fn durable_native_query(device: &str, payload: &Value) -> Result<Option<Value>, String> {
    use rusqlite::OptionalExtension;
    let db = native_ledger()?;
    let row: Option<(String,String)> = db.query_row("SELECT digest,receipt FROM actions WHERE device=?1 AND action=?2",
        rusqlite::params![device,payload["client_action_id"].as_str().unwrap_or_default()],
        |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e| e.to_string())?;
    let Some((digest, receipt)) = row else { return Ok(None); };
    if digest != native_digest(payload) { return Err("action_id_content_mismatch".into()); }
    let mut receipt: Value = serde_json::from_str(&receipt).map_err(|e| e.to_string())?;
    if receipt["payload"]["final_result"] != true { receipt = unauthoritative_result(payload, "durable_original_confirmation_unknown"); }
    Ok(Some(receipt))
}
async fn persist_saved_history(q: &Question, answer: &str) -> Result<Value, String> {
    record_native_history(q, NodeType::User, answer.to_owned()).await;
    let manager = ConversationManager::new_with_forced_persistence();
    let route = q.timeline_route();
    let tree = manager.get_tree_for_route(Some(&route), None).await.ok_or("native_history_not_persisted")?;
    let current = manager.get_current_node_id(&tree).await.ok_or("native_history_not_persisted")?;
    let nodes = manager.get_node_path(&tree, &current).await?;
    let event = native_question_history_event_id(q, &NodeType::User);
    let node = nodes.iter().find(|n| n.metadata.run_id.as_deref() == Some(event.as_str())
        && n.content == answer && n.node_type == NodeType::User && n.metadata.project_path.as_deref() == Some(q.cwd.as_str()))
        .ok_or("native_history_not_persisted")?;
    Ok(json!({"tree_id":tree,"node_id":node.id,"event_id":event,"route":route}))
}
fn action_result(
    payload: &Value,
    state: ActionState,
    authority: bool,
    reason: Option<&str>,
) -> Value {
    let delivered = matches!(state, ActionState::Saved | ActionState::CliReceived);
    json!({"message_type":"mcp_action_result","payload":{"source":"codex_native","request_id":payload.get("request_id"),"project_path":payload.get("project_path"),"client_action_id":payload.get("client_action_id"),"action":payload.get("action"),
        "status":match state{ActionState::Saved|ActionState::CliReceived=>"delivered",ActionState::Rejected=>"rejected",ActionState::Pending=>"pending",ActionState::Unknown=>"unknown"},"delivered":delivered,"final_result":authority,
        "result_kind":match state{ActionState::Saved=>Some("saved"),ActionState::CliReceived=>Some("cli_received"),_=>None},"reason":reason}})
}
pub(crate) fn unauthoritative_result(payload: &Value, reason: &str) -> Value {
    action_result(payload, ActionState::Unknown, false, Some(reason))
}
fn ledger_result(h: &Hub, key: &ActionKey) -> Option<Value> {
    h.actions.get(key).map(|a| {
        action_result(
            &a.payload,
            a.state,
            matches!(
                a.state,
                ActionState::Saved | ActionState::CliReceived | ActionState::Rejected
            ),
            a.reason.as_deref(),
        )
    })
}
fn answer_text(q: &Question, payload: &Value) -> Result<String, &'static str> {
    let allowed = [
        "action",
        "project_path",
        "request_id",
        "client_action_id",
        "user_input",
        "selected_options",
        "native_query",
    ];
    let object = payload.as_object().ok_or("invalid_payload")?;
    if object.keys().any(|k| !allowed.contains(&k.as_str()))
        || payload["action"].as_str() != Some("submit")
    {
        return Err("native_submit_only_text_options");
    }
    let text = match payload.get("user_input") {
        None => "",
        Some(v) => v.as_str().ok_or("invalid_text")?,
    };
    let mut selected = HashSet::new();
    if let Some(options) = payload.get("selected_options") {
        let options = options.as_array().ok_or("invalid_options")?;
        for value in options {
            let option = value.as_str().ok_or("invalid_options")?;
            if !q.options.iter().any(|v| v == option) {
                return Err("invalid_options");
            }
            selected.insert(option);
        }
    }
    let options = q
        .options
        .iter()
        .filter(|o| selected.contains(o.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let option_line = if options.is_empty() {
        String::new()
    } else {
        format!("选中的选项: {}", options.join("；"))
    };
    let answer = if option_line.is_empty() {
        text.to_string()
    } else if text.trim().is_empty() {
        option_line
    } else {
        format!("{option_line}\n\n{text}")
    };
    if answer.trim().is_empty() {
        return Err("empty_answer");
    }
    if answer.len() > MAX_ANSWER {
        return Err("answer_exceeds_32k_utf8");
    }
    Ok(answer)
}
pub(crate) async fn mobile_action(device: &str, payload: &Value) -> Value {
    mobile_action_with_origin(device, payload, false).await
}
pub(crate) async fn cloud_mobile_action(device: &str, payload: &Value) -> Value {
    mobile_action_with_origin(device, payload, true).await
}
async fn mobile_action_with_origin(device: &str, payload: &Value, cloud: bool) -> Value {
    let _submission = match crate::cross_device::hub_transport::submission_guard(cloud) {
        Ok(guard) => guard,
        Err(_) => return unauthoritative_result(payload, "source_submissions_frozen"),
    };
    let Some(id) = payload
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|v| v.starts_with("native-codex:"))
    else {
        return unauthoritative_result(payload, "invalid_native_identity");
    };
    let Some(path) = payload
        .get("project_path")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    else {
        return unauthoritative_result(payload, "missing_project_path");
    };
    let Some(action_id) = payload
        .get("client_action_id")
        .and_then(Value::as_str)
        .filter(|v| uuid::Uuid::parse_str(v).is_ok())
    else {
        return unauthoritative_result(payload, "invalid_action_id");
    };
    if device.is_empty() || payload["action"].as_str() != Some("submit") {
        return unauthoritative_result(payload, "native_submit_only");
    }
    let key = (device.to_owned(), action_id.to_owned());
    let body = canonical(payload);
    let queued = 'admission: {
        let mut h = lock_hub();
        if let Some(old) = h.actions.get(&key) {
            if old.payload != body {
                return unauthoritative_result(payload, "action_id_content_mismatch");
            }
            return ledger_result(&h, &key).unwrap();
        }
        match durable_native_query(device, payload) {
            Ok(Some(receipt)) => return receipt,
            Ok(None) => {},
            Err(_) => return unauthoritative_result(payload, "native_durable_ledger_unavailable_or_conflict"),
        }
        if payload.get("native_query") == Some(&Value::Bool(true)) {
            return unauthoritative_result(payload, "original_action_unknown");
        }
        if payload.get("native_query").is_some() {
            return unauthoritative_result(payload, "invalid_query_control");
        }
        let Some(q) = h
            .questions
            .iter()
            .find(|q| q.id == id && q.cwd == path && !q.hidden)
            .cloned()
        else {
            return unauthoritative_result(payload, "question_not_active");
        };
        if q.owner != h.owner || h.actions.len() >= MAX_ACTIONS {
            return unauthoritative_result(payload, "owner_or_ledger_capacity");
        }
        let answer = match answer_text(&q, &body) {
            Ok(a) => a,
            Err(r) => return unauthoritative_result(payload, r),
        };
        let group = q.group().to_string();
        let rejection = if h.group_devices.get(&group).is_some_and(|d| d != device) {
            Some("group_owned_by_another_device")
        } else if h.staged.contains_key(id) {
            Some("answer_already_saved_or_pending")
        } else if h
            .questions
            .iter()
            .any(|c| c.group() == q.group() && matches!(c.status, "sending" | "unknown"))
        {
            Some("original_submission_pending_or_unknown")
        } else {
            None
        };
        if let Some(reason) = rejection {
            h.actions.insert(
                key.clone(),
                ActionEntry {
                    payload: body,
                    ids: vec![id.into()],
                    state: ActionState::Rejected,
                    conflict: false,
                    inflight: false,
                    reason: Some(reason.into()),
                },
            );
            if persist_native(&h,&key,None).is_err(){h.actions.get_mut(&key).unwrap().state=ActionState::Unknown;return unauthoritative_result(payload,"native_ledger_write_failed");}
            return ledger_result(&h, &key).unwrap();
        }
        let current = h
            .questions
            .iter()
            .filter(|c| c.group() == q.group())
            .cloned()
            .collect::<Vec<_>>();
        let final_answer = current
            .iter()
            .all(|c| c.id == id || h.staged.get(&c.id).is_some_and(|s|s.saved));
        if current.iter().any(|c|c.id!=id && h.staged.get(&c.id).is_some_and(|s|!s.saved)) {
            return unauthoritative_result(payload,"previous_answer_not_durably_saved");
        }
        if final_answer && h.pending_sends >= MAX_SENDS {
            return unauthoritative_result(payload, "cli_send_capacity");
        }
        let Some(sender) = h.sessions.get(&q.launch).cloned() else {
            return unauthoritative_result(payload, "cli_disconnected");
        };
        h.group_devices.insert(group, device.into());
        h.staged.insert(
            id.into(),
            Stage {
                answer: answer.clone(),
                saved: false,
            },
        );
        if !final_answer {
            h.actions.insert(
                key.clone(),
                ActionEntry {
                    payload: body,
                    ids: vec![id.into()],
                    state: ActionState::Pending,
                    conflict: false,
                    inflight: false,
                    reason: None,
                },
            );
            if persist_native(&h, &key, None).is_err() {
                h.actions.get_mut(&key).unwrap().state=ActionState::Unknown;
                return unauthoritative_result(payload, "native_ledger_write_failed");
            }
            let owner = h.owner.clone();
            break 'admission Err((owner, q, answer));
        }
        let answers = current
            .iter()
            .map(|c| (c.id.clone(), h.staged[&c.id].answer.clone()))
            .collect();
        let ids = current.iter().map(|c| c.id.clone()).collect();
        h.actions.insert(
            key.clone(),
            ActionEntry {
                payload: body,
                ids,
                state: ActionState::Pending,
                conflict: false,
                inflight: true,
                reason: None,
            },
        );
        h.pending_sends += 1;
        if persist_native(&h, &key, None).is_err() {
            h.pending_sends=h.pending_sends.saturating_sub(1);
            let entry=h.actions.get_mut(&key).unwrap();entry.state=ActionState::Unknown;entry.inflight=false;
            return unauthoritative_result(payload, "native_ledger_write_failed");
        }
        for c in &mut h.questions {
            if c.group() == q.group() {
                c.status = "sending";
            }
        }
        Ok((
            h.owner.clone(),
            sender,
            Submission {
                owner: h.owner.clone(),
                question: q,
                answers,
                action_key: key.clone(),
            },
        ))
    };
    let (owner, sender, submission) = match queued {
        Ok(queued) => queued,
        Err((owner, q, answer)) => {
            let history = persist_saved_history(&q, &answer).await;
            let result = {
                let mut h = lock_hub();
                if h.owner != owner { return unauthoritative_result(payload, "owner_ended"); }
                h.actions.get_mut(&key).unwrap().state=if history.is_ok(){ActionState::Saved}else{ActionState::Unknown};
                if persist_native(&h, &key, history.as_ref().ok()).is_err() { h.actions.get_mut(&key).unwrap().state=ActionState::Unknown; }
                if h.actions[&key].state==ActionState::Saved {if let Some(stage)=h.staged.get_mut(&q.id){stage.saved=true;}}
                ledger_result(&h, &key).unwrap()
            };
            publish(&owner);
            return result;
        }
    };
    if sender.try_send(submission).is_err() {
        finish_action(&owner, &key, false, false, "cli_send_not_queued");
    }
    publish(&owner);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let changed = CHANGED.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        {
            let h = lock_hub();
            if h.owner != owner {
                return unauthoritative_result(payload, "owner_ended");
            }
            if h.actions
                .get(&key)
                .is_some_and(|a| a.state != ActionState::Pending)
            {
                return ledger_result(&h, &key).unwrap();
            }
        }
        if tokio::time::timeout_at(deadline, changed).await.is_err() {
            mark_unknown(&owner, &key, "cli_confirmation_timeout");
            return ledger_result(&lock_hub(), &key)
                .unwrap_or_else(|| unauthoritative_result(payload, "owner_ended"));
        }
    }
}
fn mark_unknown(owner: &str, key: &ActionKey, reason: &str) {
    let mut h = lock_hub();
    if h.owner != owner {
        return;
    }
    if let Some(a) = h.actions.get_mut(key) {
        if matches!(a.state, ActionState::Pending | ActionState::Unknown) {
            a.state = ActionState::Unknown;
            a.reason = Some(reason.into());
        }
    }
    if let Some(ids) = h.actions.get(key).map(|a| a.ids.clone()) {
        for q in &mut h.questions {
            if ids.contains(&q.id) {
                q.status = "unknown";
            }
        }
    }
    drop(h);
    publish(owner);
}
fn finish_action(owner: &str, key: &ActionKey, accepted: bool, uncertain: bool, reason: &str) {
    let mut h = lock_hub();
    if h.owner != owner {
        return;
    }
    let Some(entry) = h.actions.get(key) else {
        return;
    };
    let ids = entry.ids.clone();
    let conflict = entry.conflict;
    let history_answers = if accepted && !uncertain && !conflict {
        h.questions.iter().filter(|q| ids.contains(&q.id)).filter_map(|q|
            h.staged.get(&q.id).map(|stage| (q.clone(), stage.answer.clone()))).collect::<Vec<_>>()
    } else { Vec::new() };
    let Some(entry) = h.actions.get_mut(key) else { return; };
    entry.state = if uncertain || conflict {
        ActionState::Unknown
    } else if accepted {
        ActionState::CliReceived
    } else {
        ActionState::Rejected
    };
    entry.reason = if accepted && !conflict && !uncertain {
        None
    } else {
        Some(reason.into())
    };
    let inflight = entry.inflight;
    entry.inflight = false;
    if inflight {
        h.pending_sends = h.pending_sends.saturating_sub(1);
    }
    if accepted && !conflict && !uncertain {
        h.questions.retain(|q| !ids.contains(&q.id));
        for id in &ids {
            h.staged.remove(id);
        }
    } else if !uncertain && !conflict {
        let last = h
            .actions
            .get(key)
            .and_then(|a| a.payload["request_id"].as_str())
            .map(str::to_owned);
        if let Some(id) = last {
            h.staged.remove(&id);
        }
        for q in &mut h.questions {
            if ids.contains(&q.id) {
                q.status = "waiting";
            }
        }
    } else {
        for q in &mut h.questions {
            if ids.contains(&q.id) {
                q.status = "unknown";
            }
        }
    }
    let groups = h
        .questions
        .iter()
        .map(|q| q.group().to_string())
        .collect::<HashSet<_>>();
    h.group_devices.retain(|g, _| groups.contains(g));
    if persist_native(&h, key, None).is_err() {
        if let Some(entry)=h.actions.get_mut(key) { entry.state=ActionState::Unknown;entry.reason=Some("native_ledger_write_failed".into()); }
    }
    let mut event = ledger_result(&h, key).unwrap();
    event["native_device"] = json!(key.0);
    drop(h);
    for (question, answer) in history_answers {
        tokio::spawn(async move { record_native_history(&question, NodeType::User, answer).await; });
    }
    let _ = EVENTS.send(event);
    publish(owner);
}
fn invalidate(owner: &str, launch: &str, thread: Option<&str>, replied: Option<&HashSet<String>>) {
    let mut h = lock_hub();
    if h.owner != owner {
        return;
    }
    let ids = h
        .questions
        .iter()
        .filter(|q| {
            q.launch == launch
                && thread.is_none_or(|t| q.thread == t)
                && replied.is_none_or(|r| r.contains(&q.item_id()))
        })
        .map(|q| q.id.clone())
        .collect::<HashSet<_>>();
    h.questions.retain(|q| !ids.contains(&q.id));
    for id in &ids {
        h.staged.remove(id);
    }
    for a in h.actions.values_mut() {
        if matches!(a.state, ActionState::Pending | ActionState::Unknown)
            && a.ids.iter().any(|id| ids.contains(id))
        {
            a.conflict = true;
            a.state = ActionState::Unknown;
            a.reason = Some("question_closed_while_unconfirmed".into());
        }
    }
    let groups = h
        .questions
        .iter()
        .map(|q| q.group().to_string())
        .collect::<HashSet<_>>();
    h.group_devices.retain(|g, _| groups.contains(g));
}
fn add_question(q: Question) -> bool {
    let mut h = lock_hub();
    if h.owner != q.owner
        || h.issued.contains_key(&q.id)
        || h.issued.len() >= MAX_IDENTITIES
        || h.questions.len() >= MAX_QUESTIONS
    {
        return false;
    }
    let groups = h
        .questions
        .iter()
        .map(|c| c.group().to_string())
        .collect::<HashSet<_>>();
    if !groups.contains(&q.group().to_string()) && groups.len() >= MAX_GROUPS {
        return false;
    }
    h.issued.insert(q.id.clone(), q.identity());
    h.questions.push(q);
    true
}
pub(crate) struct BridgeOwner {
    owner: String,
    #[cfg(target_os = "windows")]
    task: tokio::task::JoinHandle<()>,
    #[cfg(target_os = "windows")]
    _mutex: windows::Handle,
}
impl Drop for BridgeOwner {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        self.task.abort();
        let events = {
            let mut h = lock_hub();
            if h.owner != self.owner {
                return;
            }
            let events = h
                .published
                .iter()
                .map(|(id, q)| closed_message(id, q["project_path"].as_str().unwrap_or_default()))
                .collect::<Vec<_>>();
            *h = Hub::default();
            STARTED.store(false, Ordering::Release);
            events
        };
        for event in events {
            let _ = EVENTS.send(event);
        }
        CHANGED.notify_waiters();
    }
}
pub(crate) fn start_bridge_owner() -> Option<BridgeOwner> {
    #[cfg(target_os = "windows")]
    {
        let mutex = windows::claim_owner()?;
        if STARTED.swap(true, Ordering::AcqRel) {
            return None;
        }
        let owner = uuid::Uuid::new_v4().to_string();
        *lock_hub() = Hub {
            owner: owner.clone(),
            ..Hub::default()
        };
        let task = tokio::spawn(windows::monitor(owner.clone()));
        Some(BridgeOwner {
            owner,
            task,
            _mutex: mutex,
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

fn reply_ids(item: &Value) -> HashSet<String> {
    let mut ids = HashSet::new();
    let Some(content) = item.get("content").and_then(Value::as_array) else {
        return ids;
    };
    for part in content {
        if part.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(text) = part.get("text").and_then(Value::as_str) else {
            continue;
        };
        let Some(body) = text
            .trim()
            .strip_prefix(OPEN)
            .and_then(|s| s.strip_suffix(CLOSE))
        else {
            continue;
        };
        let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(body.trim()) else {
            continue;
        };
        for e in entries {
            if e.get("answer").and_then(Value::as_str).is_none() {
                continue;
            }
            if let Some(id) = e.get("questionItemId").and_then(Value::as_str) {
                if let Ok(Value::Array(parts)) = serde_json::from_str::<Value>(id) {
                    if parts.len() == 3
                        && parts[0] == "request_user_input_async"
                        && parts[1].is_string()
                        && parts[2].as_u64().is_some()
                    {
                        ids.insert(Value::Array(parts).to_string());
                    }
                }
            }
        }
    }
    ids
}
fn reply_answers(item: &Value) -> HashMap<String, String> {
    let mut answers = HashMap::new();
    if let Some(parts) = item.get("content").and_then(Value::as_array) {
        for part in parts {
            let Some(text) = part.get("text").and_then(Value::as_str) else { continue; };
            let Some(body) = text.trim().strip_prefix(OPEN).and_then(|s| s.strip_suffix(CLOSE)) else { continue; };
            let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(body.trim()) else { continue; };
            for entry in entries {
                if let (Some(id), Some(answer)) = (entry["questionItemId"].as_str(), entry["answer"].as_str()) {
                    answers.insert(id.to_owned(), answer.to_owned());
                }
            }
        }
    }
    answers
}

fn iterate_mcp_history(item: &Value) -> Option<(String, String, Option<String>)> {
    if item["type"] != "mcpToolCall" || item["server"] != "iterate-zhi"
        || item["tool"] != "call_zhi" { return None; }
    let id = item["id"].as_str()?.to_owned();
    let message = item.pointer("/arguments/message")?.as_str()?;
    if message.trim().is_empty() { return None; }
    let options = item.pointer("/arguments/predefined_options").and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let prompt = if options.is_empty() { message.to_owned() } else {
        format!("{}\n\n可选回复：\n{}", message, options.iter().map(|option| format!("- {option}")).collect::<Vec<_>>().join("\n"))
    };
    let user = if item["status"] == "completed" && item.get("error").is_none_or(Value::is_null) {
        item.pointer("/result/content").and_then(Value::as_array)
            .and_then(|parts| parts.iter().filter_map(|part| part["text"].as_str())
                .filter_map(|text| text.strip_prefix("用户输入: ")?.rsplit_once("\n继续对话: ").map(|(input, _)| input))
                .find(|input| !input.trim().is_empty()))
            .map(str::to_owned)
    } else { None };
    Some((id, prompt, user))
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use futures_util::{SinkExt, StreamExt};
    use std::{
        path::{Path, PathBuf},
        time::Duration,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        time::Instant,
    };
    use tokio_tungstenite::tungstenite::{
        handshake::{client::generate_key, derive_accept_key},
        protocol::Role,
        Message,
    };
    use windows_sys::Win32::{
        Foundation::*,
        Security::{Authorization::ConvertSidToStringSidW, Cryptography::*, *},
        System::Threading::*,
    };
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Record {
        schema_version: u32,
        launch_id: String,
        endpoint: String,
        server_pid: u32,
        process_created: String,
        owner_sid: String,
        cwd: String,
        created_at_ms: u64,
        token_ciphertext: String,
    }
    struct Capability {
        record: Record,
        token: String,
    }
    pub(super) struct Handle(HANDLE);
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn user_sid(process: HANDLE) -> Option<String> {
        unsafe {
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
                return None;
            }
            let token = Handle(token);
            let mut size = 0;
            GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut size);
            if size == 0 || size > 65536 {
                return None;
            }
            let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
            if GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            ) == 0
            {
                return None;
            }
            let user = &*(buffer.as_ptr().cast::<TOKEN_USER>());
            let mut text = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
                return None;
            }
            let mut len = 0;
            while *text.add(len) != 0 {
                len += 1;
            }
            let result = String::from_utf16(std::slice::from_raw_parts(text, len)).ok();
            LocalFree(text.cast());
            result
        }
    }
    fn valid_process(r: &Record) -> bool {
        unsafe {
            let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, r.server_pid);
            if p.is_null() {
                return false;
            }
            let p = Handle(p);
            let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) =
                std::mem::zeroed();
            let mut code = 0;
            GetExitCodeProcess(p.0, &mut code) != 0
                && code == 259
                && GetProcessTimes(p.0, &mut c, &mut e, &mut k, &mut u) != 0
                && r.process_created.parse::<u64>().ok()
                    == Some(((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
                && user_sid(p.0).as_deref() == Some(r.owner_sid.as_str())
                && user_sid(GetCurrentProcess()).as_deref() == Some(r.owner_sid.as_str())
        }
    }
    pub(super) fn claim_owner() -> Option<Handle> {
        unsafe {
            let sid = user_sid(GetCurrentProcess())?;
            let name = wide(&format!("Local\\iterate-codex-native-owner-{sid}"));
            let raw = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if raw.is_null() {
                return None;
            }
            let handle = Handle(raw);
            if GetLastError() == ERROR_ALREADY_EXISTS {
                return None;
            }
            Some(handle)
        }
    }
    fn endpoint_ok(s: &str) -> bool {
        s.strip_prefix("ws://127.0.0.1:")
            .is_some_and(|p| p.parse::<u16>().is_ok_and(|p| p != 0))
    }
    fn load(path: &Path) -> Option<Capability> {
        use std::os::windows::fs::MetadataExt;
        let m = std::fs::symlink_metadata(path).ok()?;
        if m.len() > 65536 || m.file_attributes() & 0x400 != 0 {
            return None;
        }
        let outer: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        let r: Record = serde_json::from_value(outer.clone()).ok()?;
        if r.schema_version != 1
            || uuid::Uuid::parse_str(&r.launch_id).is_err()
            || path.file_stem()?.to_str()? != r.launch_id
            || !endpoint_ok(&r.endpoint)
            || !Path::new(&r.cwd).is_absolute()
            || r.created_at_ms > chrono::Utc::now().timestamp_millis() as u64 + 10000
            || !valid_process(&r)
        {
            return None;
        }
        let mut bytes = STANDARD.decode(&r.token_ciphertext).ok()?;
        let mut entropy = b"mytool-codex-session-v1".to_vec();
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_mut_ptr(),
        };
        let entropy = CRYPT_INTEGER_BLOB {
            cbData: entropy.len() as u32,
            pbData: entropy.as_mut_ptr(),
        };
        let mut output: CRYPT_INTEGER_BLOB = unsafe { std::mem::zeroed() };
        unsafe {
            if CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            ) == 0
            {
                return None;
            }
            let slice = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
            let protected = serde_json::from_slice::<Value>(slice).ok();
            slice.fill(0);
            LocalFree(output.pbData.cast());
            let protected = protected?;
            for f in [
                "schemaVersion",
                "launchId",
                "endpoint",
                "serverPid",
                "processCreated",
                "ownerSid",
                "cwd",
                "createdAtMs",
            ] {
                if outer.get(f) != protected.get(f) {
                    return None;
                }
            }
            let token = protected.get("token")?.as_str()?.to_string();
            if token.len() < 32
                || token.len() > 256
                || !token
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            {
                return None;
            }
            Some(Capability { record: r, token })
        }
    }
    pub(super) async fn monitor(owner: String) {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let dir = PathBuf::from(local).join("Mytool").join("CodexSessions");
        let mut tasks: HashMap<String, tokio::task::AbortHandle> = HashMap::new();
        let mut children = tokio::task::JoinSet::new();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if HUB.lock().unwrap_or_else(|e| e.into_inner()).owner != owner {
                return;
            }
            while children.try_join_next().is_some() {}
            tasks.retain(|_, t| !t.is_finished());
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten().take(512) {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let Some(launch) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)
                else {
                    continue;
                };
                if tasks.contains_key(&launch) {
                    continue;
                }
                let Some(cap) = load(&path) else {
                    continue;
                };
                let generation = owner.clone();
                let task = children.spawn(async move {
                    let id = cap.record.launch_id.clone();
                    let _ = connection(generation.clone(), cap).await;
                    {
                        let mut h = HUB.lock().unwrap_or_else(|e| e.into_inner());
                        if h.owner == generation {
                            h.sessions.remove(&id);
                        }
                    }
                    let unresolved = {
                        let h = lock_hub();
                        if h.owner != generation {
                            Vec::new()
                        } else {
                            h.actions
                                .iter()
                                .filter(|(_, a)| {
                                    a.inflight
                                        && a.ids.iter().any(|q| {
                                            h.issued.get(q).is_some_and(|i| i.launch == id)
                                        })
                                })
                                .map(|(key, _)| key.clone())
                                .collect::<Vec<_>>()
                        }
                    };
                    for key in unresolved {
                        finish_action(&generation, &key, false, true, "cli_disconnected");
                    }
                    invalidate(&generation, &id, None, None);
                    publish(&generation);
                    tokio::time::sleep(Duration::from_secs(3)).await;
                });
                tasks.insert(launch, task);
            }
        }
    }
    #[derive(Default)]
    struct Thread {
        active: bool,
        turn: Option<String>,
        since: i64,
        cwd: String,
        conversation_title: Option<String>,
        thread_preview: Option<String>,
    }
    enum Rpc {
        Initialize,
        List,
        Resume(String),
        InitialItems { thread: String, page: usize },
        Steer { action_key: ActionKey },
    }
    struct DeferredHistory {
        question: Question,
        kind: NodeType,
        content: String,
        event_id: String,
    }
    struct Pending {
        rpc: Rpc,
        deadline: Instant,
        timed_out: bool,
    }
    type Socket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;
    fn accepted_upgrade(response: &str, expected: &str) -> bool {
        let mut lines = response.split("\r\n");
        if !lines.next().is_some_and(|line| {
            line.starts_with("HTTP/1.1 ") && line.split_whitespace().nth(1) == Some("101")
        }) {
            return false;
        }
        let mut headers = HashMap::new();
        for line in lines.filter(|line| !line.is_empty()) {
            let Some((name, value)) = line.split_once(':') else {
                return false;
            };
            if headers
                .insert(name.to_ascii_lowercase(), value.trim().to_string())
                .is_some()
            {
                return false;
            }
        }
        headers
            .get("sec-websocket-accept")
            .is_some_and(|v| v == expected)
            && headers
                .get("upgrade")
                .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
            && headers.get("connection").is_some_and(|v| {
                v.split(',')
                    .any(|v| v.trim().eq_ignore_ascii_case("upgrade"))
            })
            && !headers.contains_key("sec-websocket-extensions")
            && !headers.contains_key("sec-websocket-protocol")
    }
    async fn private_connect(endpoint: &str, token: &str) -> Result<Socket, ()> {
        // tungstenite's client handshake traces the serialized Authorization header.
        // This loopback-only handshake never hands the bearer to its logging code.
        let port = endpoint
            .strip_prefix("ws://127.0.0.1:")
            .and_then(|p| p.parse::<u16>().ok())
            .filter(|p| *p != 0)
            .ok_or(())?;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .map_err(|_| ())?;
        let key = generate_key();
        let expected = derive_accept_key(key.as_bytes());
        let request=format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nAuthorization: Bearer {token}\r\n\r\n");
        stream.write_all(request.as_bytes()).await.map_err(|_| ())?;
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            if bytes.len() >= 16384 {
                return Err(());
            }
            bytes.push(stream.read_u8().await.map_err(|_| ())?);
        }
        let response = std::str::from_utf8(&bytes).map_err(|_| ())?;
        if !accepted_upgrade(response, &expected) {
            return Err(());
        }
        Ok(tokio_tungstenite::WebSocketStream::from_raw_socket(stream, Role::Client, None).await)
    }
    async fn rpc(
        s: &mut Socket,
        p: &mut HashMap<u64, Pending>,
        next: &mut u64,
        method: &str,
        params: Value,
        kind: Rpc,
    ) -> Result<(), ()> {
        *next += 1;
        let id = *next;
        p.insert(
            id,
            Pending {
                rpc: kind,
                deadline: Instant::now() + Duration::from_secs(15),
                timed_out: false,
            },
        );
        tokio::time::timeout(
            Duration::from_secs(5),
            s.send(Message::Text(
                json!({"id":id,"method":method,"params":params}).to_string(),
            )),
        )
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
    }
    fn clear_thread(owner: &str, launch: &str, thread: &str) {
        invalidate(owner, launch, Some(thread), None);
        publish(owner);
    }
    async fn connection(owner: String, cap: Capability) -> Result<(), ()> {
        let mut socket = tokio::time::timeout(
            Duration::from_secs(5),
            private_connect(&cap.record.endpoint, &cap.token),
        )
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
        let r = cap.record;
        drop(cap.token);
        log::info!("[CLI questions] connected launch={}", r.launch_id);
        let launch = &r.launch_id;
        let (tx, mut submissions) = mpsc::channel::<Submission>(32);
        {
            let mut h = HUB.lock().unwrap_or_else(|e| e.into_inner());
            if h.owner != owner {
                return Err(());
            }
            h.sessions.insert(launch.clone(), tx);
        }
        let mut pending = HashMap::new();
        let mut next = 0;
        let mut initialized = false;
        let mut threads: HashMap<String, Thread> = HashMap::new();
        let mut own = HashSet::new();
        let mut cursors = HashSet::new();
        let mut initial_pending = HashSet::<String>::new();
        let mut initial_checked = HashSet::<String>::new();
        let mut first_user_item_ids = HashMap::<String, String>::new();
        let mut recorded_reply_items = HashSet::<String>::new();
        let mut deferred_history = HashMap::<String, Vec<DeferredHistory>>::new();
        rpc(&mut socket,&mut pending,&mut next,"initialize",json!({"clientInfo":{"name":"iterate_local_questions","title":"iterate","version":"1"},"capabilities":{"experimentalApi":true}}),Rpc::Initialize).await?;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _=tick.tick()=>{
                    if !valid_process(&r)||HUB.lock().unwrap_or_else(|e|e.into_inner()).owner!=owner{break;}
                    let expired=pending.iter().filter(|(_,p)|!p.timed_out&&p.deadline<=Instant::now()).map(|(id,_)|*id).collect::<Vec<_>>();
                    for id in expired{
                        if let Some(p)=pending.get_mut(&id){if let Rpc::Steer{action_key}=&p.rpc{p.timed_out=true;mark_unknown(&owner,action_key,"cli_ack_timeout");continue;}}
                        if let Some(p)=pending.remove(&id){
                            if matches!(p.rpc,Rpc::Initialize){return Err(());}
                            if let Rpc::InitialItems{thread,..}=p.rpc {
                                initial_pending.remove(&thread);initial_checked.insert(thread.clone());
                                for event in deferred_history.remove(&thread).unwrap_or_default(){
                                    record_native_history_event(&event.question,event.kind,event.content,event.event_id).await;
                                }
                                release_initial_history(&format!("native-cli:{}:{}",launch,thread));
                            }
                        }
                    }
                    if initialized&&!pending.values().any(|p|matches!(p.rpc,Rpc::List)){cursors.clear();rpc(&mut socket,&mut pending,&mut next,"thread/loaded/list",json!({"limit":100}),Rpc::List).await?;}
                }
                Some(submit)=submissions.recv()=>{
                    let _send_guard=match crate::cross_device::hub_transport::submission_guard(true){Ok(g)=>g,Err(_)=>{mark_unknown(&owner,&submit.action_key,"source_submissions_frozen");continue;}};
                    let q=&submit.question;
                    let active=submit.owner==owner&&threads.get(&q.thread).is_some_and(|t|t.active&&t.turn.as_deref()==Some(q.turn.as_str()));
                    let current={let h=HUB.lock().unwrap_or_else(|e|e.into_inner());if h.owner!=owner{break;}h.questions.iter().filter(|c|c.group()==q.group()).cloned().collect::<Vec<_>>()};
                    if !active||current.is_empty()||current.iter().any(|c|!submit.answers.contains_key(&c.id)){
                        finish_action(&owner,&submit.action_key,false,false,"turn_or_question_inactive");continue;
                    }
                    let entries=current.iter().map(|c|json!({"answer":submit.answers[&c.id],"question":c.title,"questionItemId":c.item_id()})).collect::<Vec<_>>();
                    let message_id=uuid::Uuid::new_v4().to_string();own.insert(message_id.clone());
                    let params=json!({"threadId":q.thread,"expectedTurnId":q.turn,"clientUserMessageId":message_id,"input":[{"type":"text","text":format!("{OPEN}\n{}\n{CLOSE}",serde_json::to_string(&entries).map_err(|_|())?),"text_elements":[]}]});
                    if rpc(&mut socket,&mut pending,&mut next,"turn/steer",params,Rpc::Steer{action_key:submit.action_key}).await.is_err(){break;}
                }
                message=socket.next()=>{
                    let Some(Ok(message))=message else{break;};let text=match message{Message::Text(t)=>t,Message::Ping(p)=>{let _=socket.send(Message::Pong(p)).await;continue;},Message::Close(_)=>break,_=>continue};
                    let Ok(v)=serde_json::from_str::<Value>(&text)else{continue;};if let Some(id)=v.get("id").and_then(Value::as_u64){let Some(p)=pending.remove(&id)else{continue;};let ok=v.get("result").is_some()&&v.get("error").is_none();match p.rpc{
                        Rpc::Initialize=>{if !ok{break;}socket.send(Message::Text(json!({"method":"initialized","params":{}}).to_string())).await.map_err(|_|())?;initialized=true;}
                        Rpc::List if ok=>{
                            if let Some(ids)=v.pointer("/result/data").and_then(Value::as_array){for id in ids.iter().filter_map(Value::as_str){if threads.contains_key(id)||pending.values().any(|p|matches!(&p.rpc,Rpc::Resume(t)if t==id)){continue;}
                                rpc(&mut socket,&mut pending,&mut next,"thread/resume",json!({"threadId":id,"excludeTurns":true}),Rpc::Resume(id.to_string())).await?;}}
                            if let Some(cursor)=v.pointer("/result/nextCursor").and_then(Value::as_str){if cursors.len()<100&&cursors.insert(cursor.to_string()){rpc(&mut socket,&mut pending,&mut next,"thread/loaded/list",json!({"limit":100,"cursor":cursor}),Rpc::List).await?;}}
                        }
                    Rpc::Resume(id) if ok=>{let t=&v["result"]["thread"];if t.get("parentThreadId").is_some_and(|v|!v.is_null()){threads.insert(id,Thread::default());continue;}
                        log::info!("[CLI questions] subscribed launch={} thread={}",launch,id);
                            let conversation_title=resumed_thread_name(t,&id);
                            let thread_preview=resumed_thread_preview(t,&id);
                            threads.insert(id,Thread{active:t.pointer("/status/type").and_then(Value::as_str)==Some("active"),turn:None,since:chrono::Utc::now().timestamp_millis(),cwd:t.get("cwd").and_then(Value::as_str).unwrap_or(&r.cwd).to_string(),conversation_title,thread_preview});}
                        Rpc::InitialItems{thread,page}=>{
                            let first=v.pointer("/result/data").and_then(Value::as_array)
                                .and_then(|items|items.iter().find_map(|entry|{
                                    let item=&entry["item"];
                                    (item["type"].as_str()==Some("userMessage")).then_some(item)
                                }));
                            let next_cursor=v.pointer("/result/nextCursor").and_then(Value::as_str);
                            if ok&&first.is_none()&&page<10&&next_cursor.is_some(){
                                rpc(&mut socket,&mut pending,&mut next,"thread/items/list",json!({"threadId":thread,"limit":50,"sortDirection":"asc","cursor":next_cursor}),Rpc::InitialItems{thread,page:page+1}).await?;
                            }else{
                                initial_pending.remove(&thread);initial_checked.insert(thread.clone());
                                if let Some(item_id)=first.and_then(|item|item["id"].as_str()){
                                    first_user_item_ids.insert(thread.clone(),item_id.to_owned());
                                }
                                if let (Some(item),Some(question))=(first,deferred_history.get(&thread).and_then(|v|v.first()).map(|v|&v.question)){
                                    let content=item["content"].as_array().map(|parts|parts.iter().filter(|p|p["type"].as_str()==Some("text"))
                                        .filter_map(|p|p["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                                    if let Some(item_id)=item["id"].as_str(){
                                        if !content.trim().is_empty()&&!content.starts_with(OPEN){
                                            record_native_history_event(question,NodeType::User,content,format!("native-first-user:{}:{}:{}",launch,thread,item_id)).await;
                                        }
                                    }
                                }
                                for event in deferred_history.remove(&thread).unwrap_or_default(){
                                    if first_user_item_ids.get(&thread).is_some_and(|id|event.event_id==format!("native-user:{}:{}:{}",launch,thread,id)){continue;}
                                    record_native_history_event(&event.question,event.kind,event.content,event.event_id).await;
                                }
                                release_initial_history(&format!("native-cli:{}:{}",launch,thread));
                            }
                        }
                        Rpc::Steer{action_key}=>{finish_action(&owner,&action_key,ok,false,if ok{""}else{"cli_rejected"});}
                        _=>{}}continue;}
                    let method=v.get("method").and_then(Value::as_str).unwrap_or("");let p=&v["params"];let thread_id=p.get("threadId").and_then(Value::as_str).unwrap_or("");let Some(t)=threads.get_mut(thread_id)else{continue;};if t.since==0{continue;}
                    match method{
                        "thread/name/updated"=>{
                            if let Some(name)=p.get("threadName").filter(|name|name.is_string()||name.is_null()){
                                t.conversation_title=normalized_thread_name(name);
                                let changed=rename_pending_question_titles(&mut lock_hub(),&owner,launch,thread_id,t.conversation_title.clone());
                                if changed{publish(&owner);}
                            }
                        }
                        "turn/started"=>{clear_thread(&owner,launch,thread_id);t.active=true;t.turn=p.pointer("/turn/id").and_then(Value::as_str).map(str::to_owned);}
                        "turn/completed"|"thread/closed"=>{t.active=false;t.turn=None;clear_thread(&owner,launch,thread_id);}
                        "thread/status/changed"=>{t.active=p.pointer("/status/type").and_then(Value::as_str)==Some("active");if !t.active{t.turn=None;clear_thread(&owner,launch,thread_id);}}
                        "item/started"|"item/completed"=>{
                            let item=&p["item"];
                            if let Some((item_id,prompt,user))=iterate_mcp_history(item){
                                let emitted=if method=="item/started" {p["startedAtMs"].as_i64()} else {p["completedAtMs"].as_i64()}.unwrap_or(0);
                                if emitted<=t.since { continue; }
                                let question=Question{owner:owner.clone(),launch:launch.clone(),thread:thread_id.to_owned(),
                                    turn:p["turnId"].as_str().unwrap_or_default().to_owned(),call:item_id.clone(),index:0,
                                    id:format!("native-mcp:{}",json!([launch,thread_id,item_id])),title:prompt.clone(),options:Vec::new(),
                                    cwd:t.cwd.clone(),conversation_title:t.conversation_title.clone(),thread_preview:t.thread_preview.clone(),hidden:true,status:"history"};
                                let mut events=vec![DeferredHistory{question:question.clone(),kind:NodeType::Assistant,
                                    content:prompt,event_id:format!("native-mcp:assistant:{}:{}:{}",launch,thread_id,item_id)}];
                                if let Some(user)=user { events.push(DeferredHistory{question:question.clone(),kind:NodeType::User,
                                    content:user,event_id:format!("native-mcp:user:{}:{}:{}",launch,thread_id,item_id)}); }
                                if initial_checked.contains(thread_id){
                                    for event in events {record_native_history_event(&event.question,event.kind,event.content,event.event_id).await;}
                                }else{
                                    HISTORY_WAITING.lock().unwrap_or_else(|e|e.into_inner()).insert(question.timeline_route());
                                    deferred_history.entry(thread_id.to_owned()).or_default().extend(events);
                                    if initial_pending.insert(thread_id.to_owned()){
                                        rpc(&mut socket,&mut pending,&mut next,"thread/items/list",json!({"threadId":thread_id,"limit":50,"sortDirection":"asc"}),Rpc::InitialItems{thread:thread_id.to_owned(),page:1}).await?;
                                    }
                                }
                                continue;
                            }
                            if item.get("type").and_then(Value::as_str)==Some("userMessage"){
                                if item.get("clientId").and_then(Value::as_str).is_some_and(|id|own.contains(id)){continue;}let replied=reply_ids(item);
                                if !replied.is_empty(){
                                    let answers=reply_answers(item);
                                    let local={let h=lock_hub();h.questions.iter().filter_map(|q|
                                        (q.launch==*launch && q.thread==thread_id).then(||answers.get(&q.item_id()).map(|answer|(q.clone(),answer.clone()))).flatten()
                                    ).collect::<Vec<_>>()};
                                    if let Some(item_id)=item["id"].as_str().filter(|id|recorded_reply_items.insert(format!("{}:{}",thread_id,id))){
                                        let known=local.iter().map(|(question,_)|question.item_id()).collect::<HashSet<_>>();
                                        for (question,answer) in local {tokio::spawn(async move {record_native_history(&question,NodeType::User,answer).await;});}
                                        let mut missing=answers.into_iter().filter(|(id,answer)|!known.contains(id)&&!answer.trim().is_empty()).collect::<Vec<_>>();
                                        missing.sort_by(|left,right|left.0.cmp(&right.0));
                                        for (question_id,answer) in missing {
                                            let question=Question{owner:owner.clone(),launch:launch.clone(),thread:thread_id.to_owned(),
                                                turn:p["turnId"].as_str().unwrap_or_default().to_owned(),call:item_id.to_owned(),index:0,
                                                id:format!("native-reply:{}",json!([launch,thread_id,item_id,question_id])),title:String::new(),options:Vec::new(),
                                                cwd:t.cwd.clone(),conversation_title:t.conversation_title.clone(),thread_preview:t.thread_preview.clone(),hidden:true,status:"history"};
                                            let event_id=format!("native-reply-user:{}:{}:{}:{}",launch,thread_id,item_id,question_id);
                                            if initial_checked.contains(thread_id){
                                                tokio::spawn(async move {record_native_history_event(&question,NodeType::User,answer,event_id).await;});
                                            }else{
                                                HISTORY_WAITING.lock().unwrap_or_else(|e|e.into_inner()).insert(question.timeline_route());
                                                deferred_history.entry(thread_id.to_owned()).or_default().push(DeferredHistory{question,kind:NodeType::User,content:answer,event_id});
                                                if initial_pending.insert(thread_id.to_owned()){
                                                    rpc(&mut socket,&mut pending,&mut next,"thread/items/list",json!({"threadId":thread_id,"limit":50,"sortDirection":"asc"}),Rpc::InitialItems{thread:thread_id.to_owned(),page:1}).await?;
                                                }
                                            }
                                        }
                                    }
                                    invalidate(&owner,launch,Some(thread_id),Some(&replied));publish(&owner);
                                }else if method=="item/completed"{
                                    let emitted=p.get("completedAtMs").and_then(Value::as_i64).or_else(||v.get("emittedAtMs").and_then(Value::as_i64)).unwrap_or(0);
                                    if emitted>t.since {
                                        let content=item["content"].as_array().map(|parts|parts.iter().filter(|part|part["type"].as_str()==Some("text"))
                                            .filter_map(|part|part["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
                                        if let Some(item_id)=item["id"].as_str().filter(|_|!content.trim().is_empty()&&!content.starts_with(OPEN)){
                                            let event_id=if first_user_item_ids.get(thread_id).is_some_and(|id|id==item_id){
                                                format!("native-first-user:{}:{}:{}",launch,thread_id,item_id)
                                            }else{format!("native-user:{}:{}:{}",launch,thread_id,item_id)};
                                            let question=Question{owner:owner.clone(),launch:launch.clone(),thread:thread_id.to_owned(),
                                                turn:p["turnId"].as_str().unwrap_or_default().to_owned(),call:item_id.to_owned(),index:0,
                                                id:format!("native-user:{}",json!([launch,thread_id,item_id])),title:String::new(),options:Vec::new(),
                                                cwd:t.cwd.clone(),conversation_title:t.conversation_title.clone(),thread_preview:t.thread_preview.clone(),hidden:true,status:"history"};
                                            if initial_checked.contains(thread_id){record_native_history_event(&question,NodeType::User,content,event_id).await;}
                                            else {
                                                HISTORY_WAITING.lock().unwrap_or_else(|e|e.into_inner()).insert(question.timeline_route());
                                                deferred_history.entry(thread_id.to_owned()).or_default().push(DeferredHistory{question,kind:NodeType::User,content,event_id});
                                                if initial_pending.insert(thread_id.to_owned()){
                                                    rpc(&mut socket,&mut pending,&mut next,"thread/items/list",json!({"threadId":thread_id,"limit":50,"sortDirection":"asc"}),Rpc::InitialItems{thread:thread_id.to_owned(),page:1}).await?;
                                                }
                                            }
                                        }
                                    }
                                }continue;}
                            if item.get("type").and_then(Value::as_str)!=Some("agentMessage")||item.get("delivery").and_then(Value::as_str)!=Some("async")||!t.active{continue;}
                            // Completion of an agent item is NOT an answer. Seen IDs suppress duplicates.
                            let emitted=p.get("startedAtMs").or_else(||p.get("completedAtMs")).and_then(Value::as_i64).or_else(||v.get("emittedAtMs").and_then(Value::as_i64)).unwrap_or(0);if emitted<=t.since{continue;}
                            let Some(turn)=p.get("turnId").and_then(Value::as_str)else{continue;};if t.turn.as_deref().is_some_and(|old|old!=turn){continue;}t.turn=Some(turn.to_string());
                            let Some(call)=item.get("id").and_then(Value::as_str)else{continue;};let Some(questions)=item.get("questions").and_then(Value::as_array)else{continue;};let mut added=false;
                            for(index,q)in questions.iter().enumerate(){let Some(title)=q.get("title").and_then(Value::as_str)else{continue;};let id=format!("native-codex:{}",json!([owner,launch,thread_id,turn,call,index]));
                                let options=q.get("options").and_then(Value::as_array).map(|a|a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
                                let q=Question{owner:owner.clone(),id:id.clone(),launch:launch.clone(),thread:thread_id.to_string(),turn:turn.to_string(),call:call.to_string(),index,title:title.to_string(),options,cwd:t.cwd.clone(),conversation_title:t.conversation_title.clone(),thread_preview:t.thread_preview.clone(),hidden:false,status:"waiting"};
                                log::info!("[CLI questions] new launch={} thread={} turn={} index={}",launch,thread_id,turn,index);
                                if add_question(q.clone()) {
                                    if initial_checked.contains(thread_id){record_native_history(&q, NodeType::Assistant, q.title.clone()).await;}
                                    else {
                                        HISTORY_WAITING.lock().unwrap_or_else(|e|e.into_inner()).insert(q.timeline_route());
                                        deferred_history.entry(thread_id.to_string()).or_default().push(DeferredHistory{
                                            event_id:native_question_history_event_id(&q, &NodeType::Assistant),content:q.title.clone(),
                                            question:q.clone(),kind:NodeType::Assistant,
                                        });
                                    }
                                    added=true;
                                }}
                            if added&&!initial_checked.contains(thread_id)&&initial_pending.insert(thread_id.to_string()){
                                rpc(&mut socket,&mut pending,&mut next,"thread/items/list",json!({"threadId":thread_id,"limit":50,"sortDirection":"asc"}),Rpc::InitialItems{thread:thread_id.to_string(),page:1}).await?;
                            }
                            if added{publish(&owner);}
                        }
                        _=>{}
                    }
                }
            }
        }
        for thread in initial_pending { release_initial_history(&format!("native-cli:{}:{}",launch,thread)); }
        for (_, p) in pending {
            if let Rpc::Steer { action_key } = p.rpc {
                finish_action(&owner, &action_key, false, true, "cli_disconnected");
            }
        }
        Err(())
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn validates_upgrade_and_accept_key_without_capability_logging() {
            let response="HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: expected\r\n\r\n";
            assert!(accepted_upgrade(response, "expected"));
            assert!(!accepted_upgrade(response, "wrong-key"));
            assert!(!accepted_upgrade(
                &response.replace("101", "401"),
                "expected"
            ));
            assert!(!accepted_upgrade(
                &response.replace("websocket", "http"),
                "expected"
            ));
        }
        #[test]
        fn rejects_reused_pid_and_non_loopback() {
            let r = Record {
                schema_version: 1,
                launch_id: String::new(),
                endpoint: String::new(),
                server_pid: std::process::id(),
                process_created: "0".into(),
                owner_sid: unsafe { user_sid(GetCurrentProcess()).unwrap() },
                cwd: "C:/".into(),
                created_at_ms: 0,
                token_ciphertext: String::new(),
            };
            assert!(!valid_process(&r));
            assert!(endpoint_ok("ws://127.0.0.1:53123"));
            for s in [
                "ws://localhost:3",
                "ws://example.com:3",
                "ws://127.0.0.1:0",
                "ws://127.0.0.1:3/path",
                "wss://127.0.0.1:3",
            ] {
                assert!(!endpoint_ok(s));
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_iterate_mcp_history_keeps_options_and_full_submitted_text_only() {
        let mut item=json!({"type":"mcpToolCall","id":"exec-item","server":"iterate-zhi","tool":"call_zhi",
            "status":"inProgress","arguments":{"message":"请选","predefined_options":["A","B"]},"result":null,"error":null});
        let (_,prompt,user)=iterate_mcp_history(&item).unwrap();
        assert_eq!(prompt,"请选\n\n可选回复：\n- A\n- B");
        assert!(user.is_none());
        item["status"]=json!("completed");
        item["result"]=json!({"content":[{"type":"text","text":"用户输入: 选中的选项: A\n\n追加正文\n继续对话: true\n响应来源: popup"}]});
        assert_eq!(iterate_mcp_history(&item).unwrap().2.as_deref(),Some("选中的选项: A\n\n追加正文"));
        item["server"]=json!("other");
        assert!(iterate_mcp_history(&item).is_none());
        item["server"]=json!("iterate-zhi");
        item["error"]=json!({"message":"failed"});
        assert!(iterate_mcp_history(&item).unwrap().2.is_none());
    }
    fn question() -> Question {
        Question {
            owner: "owner".into(),
            id: "native-codex:q".into(),
            launch: "launch".into(),
            thread: "thread".into(),
            turn: "turn".into(),
            call: "call".into(),
            index: 2,
            title: "?".into(),
            conversation_title: None,
            thread_preview: None,
            options: vec!["A".into(), "B".into()],
            cwd: "C:/same".into(),
            hidden: false,
            status: "waiting",
        }
    }
    #[test]
    fn resumed_thread_title_uses_only_exact_thread_name() {
        let thread=json!({"id":"thread","name":"  对应的 CLI 会话  ","preview":"首条正文","cwd":"C:/project"});
        assert_eq!(resumed_thread_name(&thread,"thread").as_deref(),Some("对应的 CLI 会话"));
        assert!(resumed_thread_name(&thread,"other-thread").is_none());
        for name in [Value::Null,json!(" "),json!(7)] {
            let mut absent=thread.clone();absent["name"]=name;
            assert!(resumed_thread_name(&absent,"thread").is_none());
        }
        assert!(resumed_thread_name(&json!({"id":"thread","preview":"首条正文"}),"thread").is_none());
        assert_eq!(resumed_thread_name(&json!({"id":"thread","name":"null"}),"thread").as_deref(),Some("null"));
    }
    #[test]
    fn unnamed_cli_title_uses_only_verified_preview_first_sentence_and_id_suffix() {
        let id="019caabb-1234-5678-9876-11223344aabb";
        let thread=json!({"id":id,"name":null,"preview":"  会话首句。后续句子\n第二段  ","cwd":"C:/unrelated"});
        let preview=resumed_thread_preview(&thread,id);
        assert_eq!(native_conversation_title(None,preview.as_deref(),id),"会话首句。 · 3344aabb");
        assert!(resumed_thread_preview(&thread,"another-thread").is_none());
        assert_eq!(native_conversation_title(Some("  真实线程标题  "),preview.as_deref(),id),"真实线程标题");
        assert_eq!(native_conversation_title(Some("null"),preview.as_deref(),id),"null");
        assert_eq!(native_conversation_title(None,None,id),"未命名会话 · 3344aabb");
        assert_eq!(native_conversation_title(Some(" "),Some("\n  "),id),"未命名会话 · 3344aabb");
        assert_eq!(native_conversation_title(None,Some("第一段\n第二段"),id),"第一段 · 3344aabb");
        assert_ne!(native_conversation_title(None,Some("同一首句"),id),
            native_conversation_title(None,Some("同一首句"),"019caabb-1234-5678-9876-11223344ccdd"));
    }
    #[test]
    fn unnamed_cli_title_ascii_sentence_mark_preserves_file_and_url_components() {
        let id="019caabb-1234-5678-9876-11223344aabb";
        assert_eq!(native_conversation_title(None,Some("Read main.rs. Next sentence."),id),"Read main.rs. · 3344aabb");
        assert_eq!(native_conversation_title(None,Some("Check https://example.com/a?b=c. Next sentence."),id),
            "Check https://example.com/a?b=c. · 3344aabb");
        assert_eq!(native_conversation_title(None,Some("First sentence! Another sentence."),id),"First sentence! · 3344aabb");
    }
    #[test]
    fn unnamed_cli_title_bounds_unicode_preview_without_changing_exact_thread() {
        let preview="😀".repeat(70);
        let thread_id="thread-id-尾部会话对应编号";
        let expected_prefix=format!("{}…", "😀".repeat(64));
        let title=native_conversation_title(None,Some(&preview),thread_id);
        assert!(title.starts_with(&expected_prefix));
        assert_eq!(title,format!("{expected_prefix} · 尾部会话对应编号"));
        let mut q=question();q.thread=thread_id.to_owned();q.thread_preview=Some(preview);
        assert_eq!(q.public()["conversation_title"],title);
        assert_eq!(q.public()["codex_thread_id"],thread_id);
        assert_eq!(q.public()["message"],"?");
    }
    #[test]
    fn clearing_cli_name_restores_saved_preview_and_keeps_question_identity() {
        let mut q=question();q.thread_preview=Some("原始首句。其他内容".into());
        let initial=q.public();
        let mut h=Hub{owner:"owner".into(),questions:vec![q.clone()],..Hub::default()};
        assert!(rename_pending_question_titles(&mut h,"owner","launch","thread",Some("真实改名".into())));
        assert_eq!(visible(&h)[&q.id]["conversation_title"],"真实改名");
        assert!(rename_pending_question_titles(&mut h,"owner","launch","thread",None));
        assert_eq!(visible(&h)[&q.id],initial);
        assert_eq!(h.questions[0].thread_preview,q.thread_preview);
        assert_eq!(visible(&h)[&q.id]["conversation_title"],"原始首句。 · thread");
    }
    #[test]
    fn cli_conversation_title_does_not_replace_question_or_reply_identity() {
        let mut q=question();
        let identity=q.item_id();let group=q.group();let route=q.timeline_route();
        let mut before=q.public();
        q.conversation_title=Some("实际线程名称".into());
        before["conversation_title"]=json!("实际线程名称");
        assert_eq!(q.public(),before);
        assert_eq!(q.public()["message"],"?");
        assert_eq!(q.item_id(),identity);assert_eq!(q.group(),group);assert_eq!(q.timeline_route(),route);
        assert_eq!(answer_text(&q,&json!({"action":"submit","user_input":"回复原题"})).unwrap(),"回复原题");
    }
    #[test]
    fn rename_changes_only_owner_launch_thread_pending_titles_and_public_state() {
        let target=question();
        let mut other_launch=target.clone();other_launch.id="other-launch".into();other_launch.launch="second-launch".into();
        let mut other_thread=target.clone();other_thread.id="other-thread".into();other_thread.thread="second-thread".into();
        let mut h=Hub{owner:"owner".into(),questions:vec![target.clone(),other_launch.clone(),other_thread.clone()],..Hub::default()};
        let before=visible(&h);
        assert!(!rename_pending_question_titles(&mut h,"stale-owner","launch","thread",Some("不能修改".into())));
        assert_eq!(visible(&h),before);
        assert!(rename_pending_question_titles(&mut h,"owner","launch","thread",Some("改名后的线程".into())));
        let after=visible(&h);
        assert_eq!(after[&target.id]["conversation_title"],"改名后的线程");
        assert_eq!(after[&other_launch.id],before[&other_launch.id]);
        assert_eq!(after[&other_thread.id],before[&other_thread.id]);
        let mut expected=before[&target.id].clone();expected["conversation_title"]=json!("改名后的线程");
        assert_eq!(after[&target.id],expected);
        assert!(!rename_pending_question_titles(&mut h,"owner","launch","thread",Some("改名后的线程".into())));
        assert!(rename_pending_question_titles(&mut h,"owner","launch","thread",normalized_thread_name(&Value::Null)));
        assert_eq!(visible(&h)[&target.id],before[&target.id]);
    }
    #[test]
    fn history_identity_survives_bridge_owner_restart() {
        let first = question();
        let mut restarted = first.clone();
        restarted.owner = "new-owner".into();
        restarted.id = "native-codex:new-owner-id".into();
        assert_eq!(
            native_question_history_event_id(&first, &NodeType::Assistant),
            native_question_history_event_id(&restarted, &NodeType::Assistant),
        );
        assert_ne!(
            native_question_history_event_id(&first, &NodeType::Assistant),
            native_question_history_event_id(&first, &NodeType::User),
        );
        restarted.index += 1;
        assert_ne!(
            native_question_history_event_id(&first, &NodeType::Assistant),
            native_question_history_event_id(&restarted, &NodeType::Assistant),
        );
    }
    #[tokio::test]
    async fn native_ledger_stage_query_and_close_conflict_are_bounded() {
        *lock_hub() = Hub {
            owner: "owner".into(),
            ..Hub::default()
        };
        let mut first = question();
        first.id = "native-codex:first".into();
        first.index = 0;
        let mut last = first.clone();
        last.id = "native-codex:last".into();
        last.index = 1;
        assert!(add_question(first.clone()));
        assert!(add_question(last.clone()));
        let (tx, mut rx) = mpsc::channel(32);
        lock_hub().sessions.insert("launch".into(), tx);
        let payload = |id: &str, action: &str| json!({"request_id":id,"project_path":"C:/same","client_action_id":action,"action":"submit","user_input":"A"});
        let a = uuid::Uuid::new_v4().to_string();
        let body = payload(&first.id, &a);
        let saved = mobile_action("device", &body).await;
        assert_eq!(saved["payload"]["result_kind"], "saved");
        let mut query = body.clone();
        query["native_query"] = json!(true);
        let count = lock_hub().actions.len();
        assert_eq!(
            mobile_action("device", &query).await["payload"]["result_kind"],
            "saved"
        );
        assert_eq!(lock_hub().actions.len(), count);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            mobile_action("other-device", &query).await["payload"]["final_result"],
            false
        );
        let b = uuid::Uuid::new_v4().to_string();
        let final_body = payload(&last.id, &b);
        let task = tokio::spawn(async move { mobile_action("device", &final_body).await });
        let submitted = rx.recv().await.unwrap();
        assert_eq!(submitted.answers.len(), 2);
        mark_unknown("owner", &submitted.action_key, "timeout");
        let result = task.await.unwrap();
        assert_eq!(result["payload"]["status"], "unknown");
        invalidate("owner", "launch", Some("thread"), None);
        finish_action("owner", &submitted.action_key, true, false, "");
        let h = lock_hub();
        assert_eq!(h.actions.len(), 2);
        assert_eq!(h.pending_sends, 0);
        assert_eq!(
            ledger_result(&h, &submitted.action_key).unwrap()["payload"]["final_result"],
            false
        );
        assert!(h.questions.is_empty());
        drop(h);
        let mut miss = body.clone();
        miss["client_action_id"] = json!(uuid::Uuid::new_v4().to_string());
        miss["native_query"] = json!(true);
        assert_eq!(
            mobile_action("device", &miss).await["payload"]["status"],
            "unknown"
        );
        assert_eq!(lock_hub().actions.len(), 2);
        assert!(!add_question(first)); // Tombstones survive invalidation until this owner ends.
        *lock_hub() = Hub::default();
    }
    #[tokio::test]
    async fn cloud_native_durable_digest_history_reference_and_unknown_restart() {
        let dir=tempfile::tempdir().unwrap();
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR",dir.path());
        std::env::set_var("ITERATE_CONVERSATION_STATE_FILE",dir.path().join("history.json"));
        let mut q=question();q.owner="durable-owner".into();
        *lock_hub()=Hub{owner:q.owner.clone(),..Hub::default()};
        let history=persist_saved_history(&q,"durable answer only in existing history").await.unwrap();
        let payload=json!({"request_id":q.id,"project_path":q.cwd,"client_action_id":uuid::Uuid::new_v4().to_string(),"action":"submit","user_input":"durable answer only in existing history"});
        let key=("test-device".to_owned(),payload["client_action_id"].as_str().unwrap().to_owned());
        {
            let mut h=lock_hub();h.actions.insert(key.clone(),ActionEntry{payload:payload.clone(),ids:vec![q.id.clone()],state:ActionState::Saved,conflict:false,inflight:false,reason:None});
            persist_native(&h,&key,Some(&history)).unwrap();
        }
        *lock_hub()=Hub::default();
        assert_eq!(durable_native_query(&key.0,&payload).unwrap().unwrap()["payload"]["result_kind"],"saved");
        let mut changed=payload.clone();changed["user_input"]=json!("conflict");assert!(durable_native_query(&key.0,&changed).is_err());
        let db=native_ledger().unwrap();
        let row:String=db.query_row("SELECT digest||receipt||history_ref FROM actions WHERE device=?1 AND action=?2",rusqlite::params![key.0,key.1],|r|r.get(0)).unwrap();
        assert!(!row.contains("durable answer only in existing history"));
        {
            let mut h=lock_hub();h.actions.insert(key.clone(),ActionEntry{payload:payload.clone(),ids:vec![q.id],state:ActionState::Pending,conflict:false,inflight:false,reason:None});persist_native(&h,&key,None).unwrap();
        }
        *lock_hub()=Hub::default();
        assert_eq!(durable_native_query(&key.0,&payload).unwrap().unwrap()["payload"]["status"],"unknown");
        assert!(lock_hub().sessions.is_empty());
        std::env::remove_var("ITERATE_CROSS_DEVICE_DIR");std::env::remove_var("ITERATE_CONVERSATION_STATE_FILE");
    }
    #[test]
    fn cloud_native_owner_close_is_durable_and_missing_owner_is_not_terminal() {
        let dir=tempfile::tempdir().unwrap();
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR",dir.path());
        std::fs::write(dir.path().join("hub-connection.json"),json!({"endpoint":"http://127.0.0.1:18555","device_id":"fixture-win","token_env":"TEST_HUB_TOKEN"}).to_string()).unwrap();
        let q=question();let id=q.id.clone();let owner=q.owner.clone();
        *lock_hub()=Hub{owner:owner.clone(),questions:vec![q],..Hub::default()};
        publish(&owner);
        assert!(!hub_native_closed(&id).unwrap());
        // The real owner observes the question removed and publishes its close.
        lock_hub().questions.clear();publish(&owner);
        assert!(hub_native_closed(&id).unwrap());
        *lock_hub()=Hub::default();
        assert!(hub_native_closed(&id).unwrap());
        assert!(!hub_native_closed("native-codex:never-confirmed-closed").unwrap());
        std::env::remove_var("ITERATE_CROSS_DEVICE_DIR");
    }
    #[test]
    fn original_owner_instance_and_index_route() {
        let mut q = question();
        let group = q.group();
        q.owner = "next".into();
        assert_ne!(group, q.group());
        assert_eq!(q.item_id(), "[\"request_user_input_async\",\"call\",2]");
        assert!(q.public().get("token").is_none());
    }
    #[test]
    fn only_real_reply_matches() {
        let text = format!(
            "{OPEN}\n{}\n{CLOSE}",
            json!([{"answer":"A","questionItemId":"[\"request_user_input_async\",\"call\",1]"}])
        );
        assert!(reply_ids(&json!({"content":[{"type":"text","text":text}]}))
            .contains("[\"request_user_input_async\",\"call\",1]"));
        assert!(reply_ids(&json!({"type":"agentMessage"})).is_empty());
    }
    #[test]
    fn validates_native_answers_and_preserves_text() {
        let q = question();
        assert_eq!(
            answer_text(
                &q,
                &json!({"action":"submit","selected_options":["B","A","B"],"user_input":" text "})
            )
            .unwrap(),
            "选中的选项: A；B\n\n text "
        );
        assert!(answer_text(&q, &json!({"action":"submit","selected_options":["fake"]})).is_err());
        assert!(answer_text(&q, &json!({"action":"continue","user_input":"x"})).is_err());
        assert!(answer_text(&q, &json!({"action":"submit","user_input":"x","images":[]})).is_err());
    }
    #[test]
    fn query_compares_original_body() {
        let p = json!({"action":"submit","user_input":"x"});
        let mut query = p.clone();
        query["native_query"] = json!(true);
        assert_eq!(canonical(&p), canonical(&query));
        query["user_input"] = json!("y");
        assert_ne!(canonical(&p), canonical(&query));
    }
}
