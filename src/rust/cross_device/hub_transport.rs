//! Opt-in outbound cloud transport. The source remains the final reply authority.
use super::{atomic_json, directory, read_json, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{fs::{self, File, OpenOptions}, path::{Path, PathBuf}, time::Duration};

#[path = "source_watchdog.rs"]
mod source_watchdog;
use source_watchdog::{Progress, Watchdog};

#[derive(Clone, Serialize, Deserialize)]
pub struct HubConfig {
    pub endpoint: String,
    pub device_id: String,
    pub token_env: String,
    #[serde(default)]
    pub ca_certificate: Option<PathBuf>,
}

pub fn config() -> Result<Option<HubConfig>> {
    let path = directory()?.join("hub-connection.json");
    if !path.exists() { return Ok(None); }
    let config: HubConfig = read_json(&path)?;
    validate(&config)?;
    Ok(Some(config))
}

pub(crate) fn data_directory() -> Result<PathBuf> { directory() }
pub(crate) fn ready() -> bool {
    directory().ok().and_then(|d|read_json::<Value>(&d.join("hub-source-health.json")).ok())
        .is_some_and(|v|v["at"].as_i64().is_some_and(|at|at+10>=chrono::Utc::now().timestamp()))
}
pub async fn ensure_source()->Result<()> {
    if ready(){return Ok(());}
    if std::env::consts::OS=="windows"{return Err("cloud source waits for configured Bridge owner".into());}
    let dir=directory()?;
    let _start=super::lock(&dir,"hub-source-start.lock")?;
    if ready(){return Ok(());}
    let mut command=std::process::Command::new(std::env::current_exe().map_err(|e|e.to_string())?);
    command.arg("--hub-source").env("ITERATE_CROSS_DEVICE_DIR",&dir)
        .env_remove("ITERATE_CROSS_DEVICE_MIRROR").env_remove("ITERATE_CROSS_DEVICE_SOURCE")
        .env_remove("ITERATE_MCP_REQUEST_FILE").env_remove("ITERATE_RESPONSE_FILE")
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(target_os="windows")] {use std::os::windows::process::CommandExt;command.creation_flags(0x08000000);}
    let mut child=command.spawn().map_err(|_|"hub_source_start_failed")?;
    for _ in 0..30 {if ready(){return Ok(());}if child.try_wait().map_err(|e|e.to_string())?.is_some(){break;}tokio::time::sleep(Duration::from_millis(100)).await;}
    Err("hub_source_not_ready".into())
}

pub fn configure_from_stdin()->Result<()> {
    use std::io::Read;
    let mut input=String::new();std::io::stdin().take(65537).read_to_string(&mut input).map_err(|e|e.to_string())?;
    if input.len()>65536{return Err("hub_config_too_large".into());}
    let config:HubConfig=serde_json::from_str(&input).map_err(|_|"hub_config_invalid")?;validate(&config)?;
    let dir=directory()?;let _guard=super::lock(&dir,"connection.lock")?;
    if let Some(old)=self::config()? {if old.device_id!=config.device_id||old.endpoint!=config.endpoint{return Err("hub_route_change_requires_controlled_migration".into());}}
    atomic_json(&dir.join("hub-connection.json"),&config)?;
    let mut local=super::settings()?;local.enabled=true;atomic_json(&dir.join("settings.json"),&local)
}

fn validate(config: &HubConfig) -> Result<()> {
    let url = reqwest::Url::parse(&config.endpoint).map_err(|_| "invalid_hub_endpoint")?;
    let isolated = url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
    if (!isolated && url.scheme() != "https") || !url.username().is_empty()
        || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
        || url.path() != "/" || config.device_id.trim().is_empty() || config.device_id.len() > 256
        || config.token_env.is_empty() || !config.token_env.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("invalid_hub_configuration".into());
    }
    Ok(())
}

fn gate_file() -> Result<File> {
    let dir = directory()?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    OpenOptions::new().create(true).read(true).write(true).open(dir.join("hub-submit.lock")).map_err(|e| e.to_string())
}

/// Shared across popup, Bridge and recovery processes; held through a write.
/// Cloud work may drain existing jobs, while old/local admission stays paused.
pub(crate) fn submission_guard(cloud: bool) -> Result<Option<File>> {
    if config()?.is_none() { return Ok(None); }
    let guard = gate_file()?;
    guard.lock_shared().map_err(|e| e.to_string())?;
    let control = known_control()?;
    if control["frozen"] == true && !(cloud && control["drain"] == true) {
        return Err("hub_submissions_frozen".into());
    }
    Ok(Some(guard))
}

fn known_control() -> Result<Value> {
    let control: Value = read_json(&directory()?.join("hub-control.json"))
        .map_err(|_| "hub_control_unknown")?;
    if !control.get("frozen").is_some_and(Value::is_boolean) {
        return Err("hub_control_unknown".into());
    }
    Ok(control)
}

/// Local tracking is allowed while offline or frozen. This lock only protects
/// the registration layout; it never authorizes a reply or network publication.
pub(super) fn registration_guard() -> Result<(File, bool)> {
    let guard = gate_file()?;
    guard.lock_shared().map_err(|e| e.to_string())?;
    let may_publish = known_control().is_ok_and(|control| control["frozen"] == false);
    Ok((guard, may_publish))
}

pub(crate) fn legacy_paused() -> bool {
    if !matches!(config(), Ok(Some(_))) { return false; }
    read_json::<Value>(&directory().unwrap_or_default().join("hub-control.json"))
        .map_or(true, |v| v["frozen"] == true)
}

pub(crate) fn legacy_connection() -> Result<Option<File>> {
    let Some(_guard)=submission_guard(false)? else {return Ok(None)};
    let dir=directory()?.join("hub-legacy-connections");fs::create_dir_all(&dir).map_err(|e|e.to_string())?;
    let file=OpenOptions::new().create_new(true).read(true).write(true).open(dir.join(uuid::Uuid::new_v4().to_string())).map_err(|e|e.to_string())?;
    file.lock().map_err(|e|e.to_string())?;Ok(Some(file))
}
fn legacy_connections_closed()->Result<bool>{
    let path=directory()?.join("hub-legacy-connections");if !path.exists(){return Ok(true);}
    for entry in fs::read_dir(path).map_err(|e|e.to_string())?.flatten(){
        let file=OpenOptions::new().read(true).write(true).open(entry.path()).map_err(|e|e.to_string())?;
        if file.try_lock().is_err(){return Ok(false);}
    }
    Ok(true)
}
fn finalizations_quiescent()->Result<bool>{
    let path=directory()?.join("finalizing");if !path.exists(){return Ok(true);}
    for entry in fs::read_dir(path).map_err(|e|e.to_string())?.flatten(){
        let file=OpenOptions::new().read(true).write(true).open(entry.path()).map_err(|e|e.to_string())?;
        if file.try_lock().is_err(){return Ok(false);}
    }
    Ok(true)
}

struct Client {
    config: HubConfig,
    token: String,
    session: String,
    http: reqwest::Client,
    progress: Progress,
}

#[derive(Clone, Serialize, Deserialize)]
struct Binding {
    source_id: String,
    request_id: String,
    registration_key: String,
    version: u64,
    route_id: String,
    kind: String,
    project_path: String,
    wire: Value,
}
fn digest(text: &str) -> String { hex::encode(ring::digest::digest(&ring::digest::SHA256,text.as_bytes())) }
fn binding_path(key: &str) -> Result<PathBuf> { Ok(directory()?.join("hub-bindings").join(format!("{}.json",digest(key)))) }
fn text<'a>(v: &'a Value, key: &str) -> &'a str { v[key].as_str().unwrap_or_default() }

fn pairing_intent_path(id:&str)->Result<PathBuf> {
    if id.is_empty() || id.len()>256 {return Err("invalid_pairing_session".into());}
    Ok(directory()?.join("hub-pairing-intents").join(format!("{}.json",digest(id))))
}

fn failed_pairing_snapshot(intent:&Value)->Value {
    json!({"session":{"session_id":intent["session_id"],"state":"failed","expires_at":intent["expires_at"],
        "error":intent["failure"],"connected_at":null,"selected_transport_mode":null},"credential":null})
}

pub(crate) async fn issue_mobile_pairing()->Result<Value> {
    if std::env::consts::OS!="windows" {return Err("hub_pairing_windows_only".into());}
    let client=Client::new(config()?.ok_or("hub_not_configured")?)?;
    let _guard=submission_guard(false)?;
    let mut intent=crate::bridge::ws::hub_pairing_intent()?;
    let payload=client.call("/source/pairing",Some(&json!({}))).await?;
    if payload["version"]!=2 || payload["device_id"]!=client.config.device_id
        || text(&payload,"base_url").trim_end_matches('/')!=client.config.endpoint.trim_end_matches('/') {
        return Err("hub_pairing_identity_mismatch".into());
    }
    intent["session_id"]=payload["pairing_session_id"].clone();
    intent["source_id"]=json!(client.config.device_id);
    intent["endpoint"]=json!(client.config.endpoint);
    intent["expires_at"]=payload["expires_at"].clone();
    intent["settled"]=json!(false);
    atomic_json(&pairing_intent_path(text(&intent,"session_id"))?,&intent)?;
    Ok(payload)
}

pub(crate) async fn mobile_pairing_session(id:&str)->Result<Value> {
    let client=Client::new(config()?.ok_or("hub_not_configured")?)?;
    let result=client.sync_pairing(id).await?;
    // The local desktop receives progress, never the credential/hash/grant.
    Ok(result["session"].clone())
}

fn bindings() -> Result<Vec<Binding>> {
    let path=directory()?.join("hub-bindings");
    if !path.exists(){return Ok(Vec::new());}
    fs::read_dir(path).map_err(|e|e.to_string())?.map(|entry|read_json(&entry.map_err(|e|e.to_string())?.path())).collect()
}

fn mirror_wire(route:&str, source:&str, name:&str, request:&Value)->Value {
    json!({"message_type":"cross_device_mirror_state","payload":{"mirror_id":route,"request_id":route,"source":"cross_device_mirror","project_path":"",
        "origin_device_id":source,"origin_name":name,"origin_platform":"macos",
        "request":{"id":route,"project_path":"","message":request["message"],"predefined_options":request["predefined_options"],"is_markdown":request["is_markdown"]}}})
}

fn registration_binding(config: &HubConfig, reg: &super::Registration) -> Result<Binding> {
    registration_binding_for_platform(config, reg, std::env::consts::OS == "macos")
}

fn registration_binding_for_platform(config: &HubConfig, reg: &super::Registration, mac: bool) -> Result<Binding> {
    let path=binding_path(&reg.key)?;
    if path.exists(){let old:Binding=read_json(&path)?;if old.source_id!=config.device_id{return Err("hub_source_identity_changed".into());}return Ok(old);}
    let request_id=text(&reg.request,"id").to_owned();
    if request_id.is_empty() || !reg.published { return Err("hub_unpublished_request".into()); }
    let route=if mac{format!("cross-mirror:{}",digest(&format!("{}:{}:1",config.device_id,reg.key)))}else{request_id.clone()};
    let project=if mac{String::new()}else{text(&reg.request,"project_path").to_owned()};
    let wire=if mac {
        if !super::text_mirror_supported(&reg.request) {return Err("mirror_text_only".into());}
        mirror_wire(&route,&config.device_id,&reg.origin_name,&reg.request)
    }else{json!({"message_type":"mcp_state","payload":{"request":reg.request,"request_id":route,"project_path":project,"showMcpPopup":true}})};
    let binding=Binding{source_id:config.device_id.clone(),request_id,registration_key:reg.key.clone(),version:1,route_id:route,kind:if mac{"mirror"}else{"mcp"}.into(),project_path:project,wire};
    atomic_json(&path,&binding)?;
    Ok(binding)
}

// The caller holds the submission and connection guards through publication.
// An approved direct-to-hub migration can retain the original local device ID;
// the cloud binding must still use the configured source identity unchanged.
fn registration_publications(expected: &HubConfig) -> Result<Vec<Binding>> {
    registration_publications_for_platform(expected, std::env::consts::OS == "macos")
}

fn registration_publications_for_platform(expected: &HubConfig, mac: bool) -> Result<Vec<Binding>> {
    let current=config()?.ok_or("hub_not_configured")?;
    if serde_json::to_value(&current).map_err(|e|e.to_string())?
        != serde_json::to_value(expected).map_err(|e|e.to_string())? {
        return Err("hub_route_changed".into());
    }
    let local=super::settings()?;
    if !local.enabled {return Ok(Vec::new());}
    let dir=directory()?;
    let mut selected=std::collections::HashSet::new();
    let mut publications=Vec::new();
    for folder in ["requests","deferred-requests"] {
        let path=dir.join(folder);
        if !path.exists(){continue;}
        for entry in fs::read_dir(path).map_err(|e|e.to_string())?.flatten() {
            let Ok(reg)=read_json::<super::Registration>(&entry.path()) else {continue};
            if !selected.insert(reg.key.clone()) {continue;}
            let _request=reg.request_lock()?;
            // Re-read the authoritative layout under the original request lock.
            let mut reg=super::load_registration(&reg.key)?;
            if !reg.active() || reg.response_file.exists() {continue;}
            if reg.origin_device_id!=local.device_id && reg.origin_device_id!=expected.device_id {
                return Err("hub_registration_identity_mismatch".into());
            }
            if mac && !super::text_mirror_supported(&reg.request) {
                // A local-only window must not prevent unrelated text windows,
                // closures or native requests from being synchronized.
                let rejection=dir.join("hub-publication-rejections").join(format!("{}.json",digest(&reg.key)));
                if !rejection.exists() {
                    atomic_json(&rejection,&json!({"registration_key":reg.key,"source_id":expected.device_id,"reason":"mirror_text_only"}))?;
                    log::warn!("cloud publication retained locally: mirror_text_only");
                }
                continue;
            }
            let deferred_path=dir.join("deferred-requests").join(format!("{}.json",reg.key));
            let was_published=reg.published;
            reg.published=true;
            // Validate/preserve an existing binding before promoting the ledger.
            let binding=registration_binding_for_platform(expected,&reg,mac)?;
            if !was_published {
                atomic_json(&reg.path()?,&reg)?;
                if deferred_path.exists(){fs::remove_file(&deferred_path).map_err(|e|e.to_string())?;}
            }
            publications.push(binding);
        }
    }
    Ok(publications)
}

impl Client {
    async fn sync_pairing(&self,id:&str)->Result<Value> {
        let path=pairing_intent_path(id)?;
        let mut intent:Value=read_json(&path).map_err(|_|"hub_pairing_not_locally_issued")?;
        if intent["session_id"]!=id || intent["source_id"]!=self.config.device_id || intent["endpoint"]!=self.config.endpoint {
            return Err("hub_pairing_intent_mismatch".into());
        }
        if intent["settled"]==true && intent["failure"].is_string() {
            return Ok(failed_pairing_snapshot(&intent));
        }
        let mut result=self.call("/source/pairing-session",Some(&json!({"session_id":id}))).await?;
        if result["session"]["session_id"]!=id {return Err("hub_pairing_session_mismatch".into());}
        if result["credential"].is_object() && intent["settled"]!=true {
            let _guard=submission_guard(true)?;
            if let Err(error)=crate::bridge::ws::apply_hub_pairing(&intent,&result) {
                if matches!(error.as_str(),"hub_pairing_local_authority_changed"|"device_id_retired_ios"|"hub_pairing_grant_expanded"
                    |"invalid_hub_pairing_credential"|"hub_pairing_device_missing"|"invalid_hub_pairing_scopes"
                    |"invalid_hub_pairing_roots"|"invalid_hub_pairing_hash"|"invalid_hub_pairing_intent") {
                    intent["failure"]=json!(error);intent["settled"]=json!(true);
                    atomic_json(&path,&intent)?;
                    return Ok(failed_pairing_snapshot(&intent));
                }
                return Err(error);
            }
            self.call("/source/pairing-applied",Some(&json!({"session_id":id,"token_hash":result["credential"]["token_hash"]}))).await?;
            intent["settled"]=json!(true);atomic_json(&path,&intent)?;
            result=self.call("/source/pairing-session",Some(&json!({"session_id":id}))).await?;
        } else if matches!(text(&result["session"],"state"),"failed"|"expired") {
            intent["settled"]=json!(true);atomic_json(&path,&intent)?;
        }
        Ok(result)
    }
    async fn sync_pairings(&self)->Result<u64> {
        let path=directory()?.join("hub-pairing-intents");
        if !path.exists(){return Ok(0);}
        let mut errors=0;
        for entry in fs::read_dir(path).map_err(|e|e.to_string())? {
            let intent=entry.map_err(|e|e.to_string()).and_then(|entry|read_json::<Value>(&entry.path()));
            match intent {
                Ok(intent) if intent["settled"]!=true => {
                    if self.sync_pairing(text(&intent,"session_id")).await.is_err(){errors+=1;}
                }
                Ok(_)=>{},
                // Retain unreadable/corrupt records for diagnosis. One failed
                // pairing cannot block another device's authorized work.
                Err(_)=>errors+=1,
            }
        }
        Ok(errors)
    }
    async fn reject(&self,job:&Value,b:&Binding,reason:&str)->Result<()> {
        let p=&job["payload"];let payload=&p["wire"]["payload"];
        let mut receipt=json!({"message_type":if b.kind=="mirror"{"cross_device_mirror_action_result"}else{"mcp_action_result"},"payload":{"request_id":b.route_id,"project_path":b.project_path,"client_action_id":payload["client_action_id"],"action":payload["action"],"status":"rejected","delivered":false,"reason":reason}});
        if b.kind=="mirror" {receipt["payload"]["mirror_id"]=json!(b.route_id);receipt["payload"]["source"]=json!("cross_device_mirror");receipt["payload"]["result_kind"]=json!("rejected");}
        if b.kind=="native" {receipt["payload"]["source"]=json!("codex_native");receipt["payload"]["final_result"]=json!(true);}
        let confirmation=json!({"action_id":p["action_id"],"source_request_id":b.request_id,"version":b.version,"registration_key":b.registration_key,"epoch":job["epoch"],"state":"rejected","durable":true,"receipt":receipt});
        let path=directory()?.join("hub-action-rejections").join(format!("{}.json",digest(text(p,"action_id"))));
        // A source permission/input refusal happened before execution. Preserve
        // that decision across reconnection without changing any winner record.
        atomic_json(&path,&json!({"wire_digest":digest(&p["wire"].to_string()),"confirmation":confirmation}))?;
        self.call("/source/confirm",Some(&confirmation)).await?;Ok(())
    }
    fn new(config: HubConfig) -> Result<Self> {
        validate(&config)?;
        let token = std::env::var(&config.token_env).map_err(|_| "hub_source_credential_missing")?;
        if token.is_empty() { return Err("hub_source_credential_missing".into()); }
        let mut builder = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20));
        if let Some(path) = &config.ca_certificate {
            let bytes = fs::read(path).map_err(|_| "hub_ca_unreadable")?;
            let cert = reqwest::Certificate::from_pem(&bytes).map_err(|_| "hub_ca_invalid")?;
            builder = builder.add_root_certificate(cert);
        }
        Ok(Self { config, token, session: uuid::Uuid::new_v4().to_string(),
            http: builder.build().map_err(|_| "hub_tls_client_failed")?, progress: Progress::new() })
    }

    async fn call(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        self.progress.enter("hub_http");
        let url = format!("{}{}", self.config.endpoint.trim_end_matches('/'), path);
        let request = if let Some(body) = body { self.http.post(url).json(body) } else { self.http.get(url) };
        let response = request.bearer_auth(&self.token)
            .header("X-Iterate-Device-Id", &self.config.device_id)
            .header("X-Iterate-Client-Kind", std::env::consts::OS)
            .header("X-Iterate-Source-Session", &self.session)
            .send().await.map_err(|_| "hub_connection_unknown")?;
        if !response.status().is_success() { return Err(format!("hub_http_{}", response.status().as_u16())); }
        if response.content_length().is_some_and(|n| n > 2 * 1024 * 1024) { return Err("hub_reply_too_large".into()); }
        response.json().await.map_err(|_| "hub_invalid_response".into())
    }

    async fn control(&self, next: &Value) -> Result<()> {
        self.progress.enter("control_lock");
        let value = json!({"epoch":next["epoch"],"frozen":next["frozen"],"drain":next["drain"],
            "freeze_generation":next["freeze_generation"],"source_generation":next["source_generation"]});
        tokio::task::spawn_blocking(move || {
            let guard = gate_file()?;
            guard.lock().map_err(|e| e.to_string())?;
            atomic_json(&directory()?.join("hub-control.json"), &value)
        }).await.map_err(|e| e.to_string())??;
        Ok(())
    }

    async fn publish(&self, epoch: &Value) -> Result<()> {
        // Publishing new work is forbidden even during a frozen drain. Keep
        // the route and control stable through the authenticated cloud writes.
        self.progress.enter("publication_locks");
        let _admission=submission_guard(false)?;
        let _route=super::lock(&directory()?,"connection.lock")?;
        for binding in registration_publications(&self.config)? {
            self.publish_binding(&binding,epoch).await?;
        }
        for binding in bindings()? {
            self.progress.enter("publication_cleanup");
            if binding.kind=="native" {
                if crate::codex_questions::hub_native_closed(&binding.request_id)? {
                    self.call("/source/close",Some(&json!({"source_request_id":binding.request_id,"version":binding.version,"registration_key":binding.registration_key}))).await?;
                }
                continue;
            }
            let reg=super::load_registration(&binding.registration_key)?;
            if !reg.active() || reg.response_file.exists(){self.call("/source/close",Some(&json!({"source_request_id":binding.request_id,"version":binding.version,"registration_key":binding.registration_key}))).await?;}
        }
        if !super::settings()?.enabled {return Ok(());}
        for wire in crate::codex_questions::snapshot(){
            let request=&wire["payload"]["request"];
            let id=text(request,"id");if id.is_empty(){continue;}
            let path=binding_path(id)?;
            let binding=if path.exists(){read_json(&path)?}else{
                let b=Binding{source_id:self.config.device_id.clone(),request_id:id.into(),registration_key:id.into(),version:1,route_id:id.into(),kind:"native".into(),project_path:text(request,"project_path").into(),wire};
                atomic_json(&path,&b)?;b
            };
            self.publish_binding(&binding,epoch).await?;
        }
        Ok(())
    }
    async fn publish_binding(&self,binding:&Binding,epoch:&Value)->Result<()> {
        self.progress.enter("publication_images");
        let mut wire=binding.wire.clone();
        if binding.kind=="mcp" {
            for mut image in crate::bridge::ws::hub_display_images(&mut wire)? {
                image["epoch"]=epoch.clone();image["source_request_id"]=json!(binding.request_id);image["version"]=json!(binding.version);
                let global=self.call("/source/image",Some(&image)).await?;
                let old=format!("/image?id={}",text(&image,"original_asset_id"));
                let replacement=format!("/image?id={}",text(&global,"id"));
                wire=serde_json::from_str(&wire.to_string().replace(&old,&replacement)).map_err(|e|e.to_string())?;
            }
        }
        self.call("/source/publish",Some(&json!({"source_request_id":binding.request_id,"version":binding.version,
            "registration_key":binding.registration_key,"route_id":binding.route_id,"kind":binding.kind,
            "project_path":binding.project_path,"epoch":epoch,"published":true,"wire":wire}))).await?;
        Ok(())
    }

    fn binding(&self, payload:&Value)->Result<Binding>{
        let b:Binding=read_json(&binding_path(text(payload,"registration_key"))?)?;
        if b.source_id!=self.config.device_id || payload["source_request_id"]!=b.request_id || payload["version"]!=b.version
            || payload["registration_key"]!=b.registration_key {return Err("hub_wrong_source_binding".into());}
        Ok(b)
    }

    async fn action(&self,job:&Value,query_only:bool)->Result<()> {
        self.progress.enter("action_binding");
        let p=&job["payload"];
        let b=self.binding(p)?;
        let principal=&p["principal"];
        let wire=&p["wire"];
        if text(&wire["payload"],"request_id")!=b.route_id {return Err("hub_wrong_wire_route".into());}
        let rejection_path=directory()?.join("hub-action-rejections").join(format!("{}.json",digest(text(p,"action_id"))));
        if rejection_path.exists(){
            let record:Value=read_json(&rejection_path)?;
            if record["wire_digest"]!=digest(&p["wire"].to_string()){return Err("hub_action_content_conflict".into());}
            self.call("/source/confirm",Some(&record["confirmation"])).await?;return Ok(());
        }
        if crate::bridge::ws::authorize_hub_principal(principal,"session.respond",&b.project_path,b.kind=="mirror").await.is_err(){
            if query_only {return Err("hub_reconcile_permission_unknown".into());}
            return self.reject(job,&b,"source_permission_denied").await;
        }
        let mut confirmation=json!({"action_id":p["action_id"],"source_request_id":b.request_id,"version":b.version,"registration_key":b.registration_key,"epoch":job["epoch"],"state":"unknown"});
        let payload=&wire["payload"];
        if b.kind=="native" {
            self.progress.enter("native_action");
            let receipt=if query_only {crate::codex_questions::durable_native_query(text(principal,"device_id"),payload)?}
                else {self.call("/source/authorize",Some(&json!({"job_id":job["id"]}))).await?;Some(crate::codex_questions::cloud_mobile_action(text(principal,"device_id"),payload).await)};
            if let Some(receipt)=receipt {
                if receipt["payload"]["final_result"]==true {
                    confirmation["state"]=json!(if receipt["payload"]["delivered"]==true{"source_accepted"}else{"rejected"});
                    confirmation["durable"]=json!(true);confirmation["winner_record"]=json!(format!("native:{}:{}",text(principal,"device_id"),text(payload,"client_action_id")));
                    confirmation["original_response"]=receipt.clone();confirmation["receipt"]=receipt;
                }
            }
        }else{
            let reg=super::load_registration(&b.registration_key)?;
            let response=if b.kind=="mirror" && payload["action"]=="close_mirror" { Value::Null }else if b.kind=="mirror"{
                let binding=super::MobileMirrorBinding{key:reg.key.clone(),revision:String::new(),request:reg.request.clone(),origin_device_id:Some(reg.origin_device_id.clone()),origin_name:Some(reg.origin_name.clone()),origin_platform:reg.origin_platform.clone()};
                match super::mobile_mirror_response(payload,&binding){Ok(response)=>response,Err(_)=>return self.reject(job,&b,"invalid_mirror_response").await}
            }else{match super::android_action_response(payload,&b.request_id,&b.project_path){Ok(response)=>response,Err(_)=>return self.reject(job,&b,"invalid_source_response").await}};
            if !query_only {
                self.call("/source/authorize",Some(&json!({"job_id":job["id"]}))).await?;
                self.progress.enter("action_commit");
                let _guard=submission_guard(true)?;
                // Existing source lock and exact full response comparison arbitrate.
                if payload["action"]=="close_mirror" && b.kind=="mirror" {
                    let _lock=reg.request_lock()?;
                    let path=directory()?.join("hub-mirror-closures").join(format!("{}.json",digest(&format!("{}:{}",reg.key,text(principal,"device_id")))));
                    let record=json!({"wire":wire,"registration_key":reg.key});
                    if path.exists() && read_json::<Value>(&path)?!=record{return Err("mirror_close_conflict".into());}
                    atomic_json(&path,&record)?;
                }else{let _=super::commit_inner(&reg,&response,true);}
            }
            self.progress.enter("action_receipt_lock");
            let _lock=reg.request_lock()?;
            let close_path=directory()?.join("hub-mirror-closures").join(format!("{}.json",digest(&format!("{}:{}",reg.key,text(principal,"device_id")))));
            if b.kind=="mirror" && payload["action"]=="close_mirror" && close_path.exists(){
                let old:Value=read_json(&close_path)?;
                if old["wire"]!=*wire{return Err("mirror_close_conflict".into());}
                confirmation["state"]=json!("source_accepted");confirmation["durable"]=json!(true);confirmation["winner_record"]=json!(format!("mirror-close:{}:{}",reg.key,text(principal,"device_id")));confirmation["original_response"]=json!({"mirror_closed":true});
                confirmation["receipt"]=json!({"message_type":"cross_device_mirror_action_result","payload":{"mirror_id":b.route_id,"request_id":b.route_id,"client_action_id":payload["client_action_id"],"action":"close_mirror","source":"cross_device_mirror","status":"accepted","result_kind":"mirror_closed"}});
            }else if reg.result_path()?.exists(){
                let winner:Value=read_json(&reg.result_path()?)?;
                if winner["response"]==response {
                    let delivered=b.kind=="mirror"||winner["source_consumed"]==true;
                    if delivered {
                        let receipt=if b.kind=="mirror"{json!({"message_type":"cross_device_mirror_action_result","payload":{"mirror_id":b.route_id,"request_id":b.route_id,"client_action_id":payload["client_action_id"],"action":payload["action"],"source":"cross_device_mirror","status":"accepted","result_kind":"mac_accepted"}})}else{json!({"message_type":"mcp_action_result","payload":{"request_id":b.route_id,"project_path":b.project_path,"client_action_id":payload["client_action_id"],"action":payload["action"],"status":"delivered","delivered":true,"reason":null,"method":"cross_device_source"}})};
                        confirmation["state"]=json!("source_accepted");confirmation["durable"]=json!(true);confirmation["winner_record"]=json!(reg.key);confirmation["original_response"]=response;confirmation["receipt"]=receipt;
                    }
                }else{
                    confirmation["state"]=json!("rejected");confirmation["durable"]=json!(true);
                    confirmation["receipt"]=json!({"message_type":if b.kind=="mirror"{"cross_device_mirror_action_result"}else{"mcp_action_result"},"payload":{"request_id":b.route_id,"mirror_id":b.route_id,"project_path":b.project_path,"client_action_id":payload["client_action_id"],"action":payload["action"],"status":"rejected","delivered":false,"result_kind":"rejected"}});
                    if b.kind=="mirror" {confirmation["receipt"]["payload"]["source"]=json!("cross_device_mirror");}
                }
            } else if query_only {
                // The original request lock excludes every source winner write.
                confirmation["state"]=json!("not_executed");confirmation["durable"]=json!(true);confirmation["definitely_not_executed"]=json!(true);
            }
        }
        self.call("/source/confirm",Some(&confirmation)).await?;
        Ok(())
    }

    async fn jobs(&self,next:&Value)->Result<()> {
        for (key,query) in [("jobs",false),("reconcile",true)] {
            for job in next[key].as_array().into_iter().flatten(){
                self.progress.enter("job_dispatch");
                if job["kind"]=="action" {self.action(job,query).await?;}
                else {
                    let path=directory()?.join("hub-rpc-receipts").join(format!("{}.json",digest(text(job,"id"))));
                    let mut response=if path.exists(){read_json(&path)?}else if query{return Err("hub_rpc_confirmation_unknown".into());}else{
                        self.call("/source/authorize",Some(&json!({"job_id":job["id"]}))).await?;
                        self.progress.enter("local_rpc");
                        let response=crate::bridge::ws::hub_rpc(&job["payload"]).await?;
                        atomic_json(&path,&response)?;response
                    };
                    // Upgrade a previously persisted, unconfirmed timeline
                    // receipt without executing its RPC a second time.
                    if job["payload"]["operation"]=="timeline" {
                        let body=&job["payload"]["wire"]["payload"];
                        for event in response["events"].as_array_mut().into_iter().flatten() {
                            let p=&mut event["payload"];
                            if p["request_id"]!=body["request_id"] || p["project_path"]!=body["project_path"] {return Err("hub_timeline_identity_mismatch".into());}
                            p["timeline_route_id"]=body["request_id"].clone();
                        }
                        atomic_json(&path,&response)?;
                    }
                    self.call("/source/respond",Some(&json!({"job_id":job["id"],"epoch":job["epoch"],"response":response}))).await?;
                }
            }
        }
        Ok(())
    }

    async fn process_next(&self, dir: &Path, next: &Value) -> Result<()> {
        // A failed control/health write aborts this round before any work.
        // Retrying must keep the source owner and session, not bypass control.
        self.control(next).await?;
        self.progress.enter("pairing_sync");
        let pairing_errors = self.sync_pairings().await.unwrap_or(1);
        atomic_json(&dir.join("hub-source-health.json"), &json!({
            "pid": std::process::id(), "device_id": self.config.device_id,
            "transport": "cloud_hub", "session": self.session,
            "at": chrono::Utc::now().timestamp(), "epoch": next["epoch"],
            "pairing_errors": pairing_errors
        }))?;
        let jobs = self.jobs(next).await;
        if next["frozen"] != true { let _ = self.publish(&next["epoch"]).await; }
        if jobs.is_ok() && next["frozen"] == true && next["drain"] == false
            && legacy_connections_closed()? && finalizations_quiescent()?
            && crate::codex_questions::hub_native_quiescent() {
            let _ = self.call("/source/barrier", Some(&json!({
                "epoch": next["epoch"], "freeze_generation": next["freeze_generation"],
                "source_generation": next["source_generation"],
                "legacy_connections_closed": true, "all_submit_paths_paused": true
            }))).await;
        }
        Ok(())
    }
}

/// A dedicated source owner shares a persistent profile with actual product
/// requests. No default route or production profile is changed by this mode.
pub fn run() -> anyhow::Result<()> {
    tokio::runtime::Runtime::new()?.block_on(run_async(None)).map_err(anyhow::Error::msg)
}
pub(crate) fn start_in_bridge_owner() -> Option<tokio::task::JoinHandle<()>> {
    if !matches!(config(),Ok(Some(_))){return None;}
    Some(tokio::spawn(async {
        use futures_util::FutureExt;
        let watchdog_limit = is_supervised_bridge().then(|| Duration::from_secs(120));
        let result = std::panic::AssertUnwindSafe(run_async(watchdog_limit)).catch_unwind().await;
        // A second source owner must never terminate the process owning a live one.
        if matches!(&result, Ok(Err(error)) if error == "hub_source_already_running") { return; }
        if is_supervised_bridge() {
            if let Ok(dir) = directory() {
                source_watchdog::record_incident(&dir, "source_task_stopped", Duration::ZERO);
            }
            std::process::exit(source_watchdog::RESTART_EXIT_CODE);
        }
        log::warn!("cloud source stopped; inspect source health");
    }))
}
fn is_supervised_bridge() -> bool {
    source_watchdog::supervised_bridge(cfg!(target_os = "windows"), std::env::args())
}
async fn run_async(watchdog_limit: Option<Duration>) -> Result<()> {
    let config = config()?.ok_or("hub_not_configured")?;
    let dir = directory()?;
    fs::create_dir_all(&dir).map_err(|e|e.to_string())?;
    let owner = OpenOptions::new().create(true).read(true).write(true).open(dir.join("hub-source.lock")).map_err(|e|e.to_string())?;
    owner.try_lock().map_err(|_| "hub_source_already_running")?;
    let client = Client::new(config)?;
    let _watchdog = if let Some(limit) = watchdog_limit {
        Some(Watchdog::start(dir.clone(), client.progress.clone(), limit, Duration::from_secs(5).min(limit / 4))
            .map_err(|_| "hub_watchdog_start_failed")?)
    } else { None };
    loop {
        if let Ok(next) = client.call("/source/next", None).await {
            if client.process_next(&dir, &next).await.is_err() {
                // Never log errors that may include credentials or user bodies.
                log::warn!("cloud source round failed; retrying with the same owner");
            }
        }
        // Even an offline/unauthorized poll is progress: do not restart a healthy
        // retry loop merely because the remote server cannot currently be reached.
        client.progress.enter("poll_complete");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_loop_watchdog_child_fixture() {
        let Ok(mode) = std::env::var("ITERATE_SOURCE_WATCHDOG_FIXTURE") else { return; };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let result = tokio::time::timeout(Duration::from_secs(5), run_async(Some(Duration::from_secs(2)))).await;
            assert!(result.is_err(), "source loop returned instead of retrying");
            assert_ne!(mode, "blocked", "blocked loop was not stopped by watchdog");
        });
    }

    #[test]
    fn source_loop_lock_stall_restarts_and_offline_retries_keep_owner_alive() {
        use std::{io::{BufRead, Write}, sync::{Arc, atomic::{AtomicBool, Ordering}}};
        let dir = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let offline = Arc::new(AtomicBool::new(false));
        struct StopServer(Arc<AtomicBool>);
        impl Drop for StopServer { fn drop(&mut self) { self.0.store(true, Ordering::Release); } }
        let _server_guard = StopServer(stop.clone());
        let server_offline = offline.clone();
        let server = std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                    let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                        if line == "\r\n" { break; }
                        line.clear();
                    }
                    let (status, body) = if server_offline.load(Ordering::Acquire) { (503, "{}") }
                        else { (200, "{\"epoch\":1,\"frozen\":false,\"drain\":false,\"jobs\":[],\"reconcile\":[]}") };
                    let _ = write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                } else { std::thread::sleep(Duration::from_millis(10)); }
            }
        });
        atomic_json(&dir.path().join("hub-connection.json"), &HubConfig {
            endpoint, device_id: "isolated-watchdog".into(), token_env: "ITERATE_SOURCE_WATCHDOG_TOKEN".into(), ca_certificate: None,
        }).unwrap();
        atomic_json(&dir.path().join("settings.json"), &json!({"device_id":"isolated-watchdog","enabled":false})).unwrap();
        let preserved = dir.path().join("results/accepted.json");
        atomic_json(&preserved, &json!({"response":{"user_input":"durable fixture"}})).unwrap();
        let run_child = |mode: &str| {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["source_loop_watchdog_child_fixture", "--nocapture", "--test-threads=1"])
                .env("ITERATE_SOURCE_WATCHDOG_FIXTURE", mode)
                .env("ITERATE_SOURCE_WATCHDOG_TOKEN", "isolated-not-a-real-secret")
                .env("ITERATE_CROSS_DEVICE_DIR", dir.path())
                .env("ITERATE_CONFIG_DIR", dir.path().join("config"))
                .env("ITERATE_CONVERSATION_STATE_FILE", dir.path().join("history.json"))
                .spawn().unwrap();
            let pid = child.id();
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            loop {
                if let Some(status) = child.try_wait().unwrap() { break (pid, status.code()); }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill(); let _ = child.wait(); panic!("source fixture timed out: {mode}");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let lock = OpenOptions::new().create(true).read(true).write(true).open(dir.path().join("hub-submit.lock")).unwrap();
        lock.lock().unwrap();
        let (blocked_pid, code) = run_child("blocked");
        assert_eq!(code, Some(source_watchdog::RESTART_EXIT_CODE));
        let incident: Value = read_json(&dir.path().join("hub-source-watchdog.json")).unwrap();
        assert_eq!(incident["pid"], blocked_pid);
        assert_eq!(incident["stage"], "control_lock");
        drop(lock);
        let (recovered_pid, code) = run_child("recovered");
        assert_eq!(code, Some(0));
        let health: Value = read_json(&dir.path().join("hub-source-health.json")).unwrap();
        assert_eq!(health["pid"], recovered_pid);
        assert_eq!(read_json::<Value>(&preserved).unwrap()["response"]["user_input"], "durable fixture");
        offline.store(true, Ordering::Release);
        let (_, code) = run_child("offline");
        assert_eq!(code, Some(0), "503 retries must not trigger restart");
        assert_eq!(read_json::<Value>(&dir.path().join("hub-source-watchdog.json")).unwrap(), incident);
        println!("source lock stall exited 75; same profile recovered with a new PID and preserved receipt; 503 retry loop survived beyond watchdog deadline");
        drop(_server_guard);
        server.join().unwrap();
    }

    #[test]
    fn cloud_owner_recovers_from_control_and_health_write_failures() {
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}, Mutex};
        struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for RestoreEnv {
            fn drop(&mut self) {
                for (key, previous) in &self.0 {
                    if let Some(value) = previous { std::env::set_var(key, value); }
                    else { std::env::remove_var(key); }
                }
            }
        }
        let _restore = RestoreEnv(["ITERATE_CROSS_DEVICE_DIR", "ITERATE_CONFIG_DIR",
            "TEST_LIVENESS_TOKEN"].into_iter().map(|key| (key, std::env::var_os(key))).collect());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for blocked in ["hub-control.json", "hub-source-health.json"] {
                let dir = tempfile::tempdir().unwrap();
                std::env::set_var("ITERATE_CROSS_DEVICE_DIR", dir.path());
                std::env::set_var("ITERATE_CONFIG_DIR", dir.path().join("config"));
                std::env::set_var("TEST_LIVENESS_TOKEN", "isolated-liveness-token");
                let polls = Arc::new(AtomicUsize::new(0));
                let work = Arc::new(AtomicUsize::new(0));
                let sessions = Arc::new(Mutex::new(std::collections::HashSet::new()));
                let (p, w, s) = (polls.clone(), work.clone(), sessions.clone());
                let app = axum::Router::new().route("/*path", axum::routing::any(
                    move |uri: axum::http::Uri, headers: axum::http::HeaderMap| {
                        let (p, w, s) = (p.clone(), w.clone(), s.clone());
                        async move {
                            assert_eq!(headers["authorization"], "Bearer isolated-liveness-token");
                            assert_eq!(headers["x-iterate-device-id"], "isolated-liveness-source");
                            s.lock().unwrap().insert(headers["x-iterate-source-session"].to_str().unwrap().to_owned());
                            let response = if uri.path() == "/source/next" {
                                p.fetch_add(1, Ordering::SeqCst);
                                json!({"epoch":1,"frozen":false,"drain":false,"jobs":[
                                    {"id":"isolated-noop","kind":"rpc","payload":{"operation":"isolated-invalid-operation"}}
                                ],"reconcile":[]})
                            } else {
                                w.fetch_add(1, Ordering::SeqCst);
                                json!({})
                            };
                            axum::Json(response)
                        }
                    }));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let endpoint = format!("http://{}", listener.local_addr().unwrap());
                let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
                atomic_json(&dir.path().join("hub-connection.json"), &HubConfig {
                    endpoint, device_id: "isolated-liveness-source".into(),
                    token_env: "TEST_LIVENESS_TOKEN".into(), ca_certificate: None
                }).unwrap();
                atomic_json(&dir.path().join("settings.json"), &super::super::Settings {
                    device_id: "isolated-liveness-source".into(), enabled: false
                }).unwrap();
                let bad = dir.path().join(blocked);
                fs::create_dir(&bad).unwrap();
                let owner = start_in_bridge_owner().unwrap();
                tokio::time::sleep(Duration::from_millis(2200)).await;
                assert!(!owner.is_finished(), "cloud owner stopped after {blocked} write failure");
                assert!(polls.load(Ordering::SeqCst) >= 2, "cloud poll did not retry");
                assert_eq!(work.load(Ordering::SeqCst), 0, "work ran before control/health persisted");
                fs::remove_dir(&bad).unwrap();
                let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
                loop {
                    if work.load(Ordering::SeqCst) > 0 && ready() { break; }
                    assert!(tokio::time::Instant::now() < deadline, "cloud owner failed to recover");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                let health: Value = read_json(&dir.path().join("hub-source-health.json")).unwrap();
                let sessions = sessions.lock().unwrap();
                assert_eq!(sessions.len(), 1, "recovery replaced the source session");
                assert!(sessions.contains(health["session"].as_str().unwrap()));
                assert_eq!(health["pid"], std::process::id());
                assert!(!owner.is_finished());
                drop(sessions);
                owner.abort();
                let _ = owner.await;
                server.abort();
                let _ = server.await;
            }
        });
    }

    fn guarded_publications(config:&HubConfig)->Result<Vec<Binding>> {
        let _admission=submission_guard(false)?;
        let _route=super::super::lock(&directory()?,"connection.lock")?;
        registration_publications(config)
    }

    #[test]
    fn cold_tracking_and_control_linearize_without_losing_concurrent_requests() {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        let previous_mirror = std::env::var_os("ITERATE_CROSS_DEVICE_MIRROR");
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR", dir.path());
        std::env::remove_var("ITERATE_CROSS_DEVICE_MIRROR");
        let config = HubConfig { endpoint: "http://127.0.0.1:18555".into(),
            device_id: "concurrent-source".into(), token_env: "TEST_HUB_TOKEN".into(), ca_certificate: None };
        atomic_json(&dir.path().join("hub-connection.json"), &config).unwrap();
        atomic_json(&dir.path().join("settings.json"), &super::super::Settings {
            device_id: config.device_id.clone(), enabled: true,
        }).unwrap();
        atomic_json(&dir.path().join("hub-source-health.json"), &json!({"at": chrono::Utc::now().timestamp()})).unwrap();
        atomic_json(&dir.path().join("hub-control.json"), &json!({"frozen":false,"drain":false})).unwrap();
        let register_one = |index: usize| {
            let path = dir.path().to_path_buf();
            std::thread::spawn(move || {
                let request = json!({"id":format!("concurrent-{index}"),"message":"pending text"});
                let request_file = path.join(format!("request-{index}.json"));
                atomic_json(&request_file, &request).unwrap();
                tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
                    .block_on(super::super::register(&request, &request_file, &path.join(format!("response-{index}.json"))))
                    .expect("concurrent local registration must survive freeze")
            })
        };
        // Hold the route lock until register is waiting with its shared control
        // lock. The real control writer must wait for that registration to finish.
        let route = super::super::lock(dir.path(), "connection.lock").unwrap();
        let first = register_one(0);
        let probe = gate_file().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while probe.try_lock().is_ok() {
            probe.unlock().unwrap();
            assert!(std::time::Instant::now() < deadline, "registration did not acquire shared control lock");
            std::thread::sleep(Duration::from_millis(5));
        }
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let control_config = config.clone();
        let writer = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            runtime.block_on(async {
                let client = Client { config: control_config, token: "isolated-test-token".into(),
                    session: "isolated-test-session".into(), http: reqwest::Client::new(), progress: Progress::new() };
                client.control(&json!({"frozen":true,"drain":false,"epoch":1})).await.unwrap();
            });
            done_tx.send(()).unwrap();
        });
        assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err(), "freeze overtook a registration write");
        assert_eq!(read_json::<Value>(&dir.path().join("hub-control.json")).unwrap()["frozen"], false);
        drop(route);
        let admitted_before_freeze = first.join().unwrap();
        assert!(admitted_before_freeze.published);
        done_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        writer.join().unwrap();
        let jobs: Vec<_> = (1..=32).map(register_one).collect();
        let pending: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
        let unique: std::collections::HashSet<_> = pending.iter().map(|reg| reg.key.clone()).collect();
        assert_eq!(unique.len(), 32);
        for reg in &pending {
            assert!(!reg.published);
            assert_eq!(super::super::load_registration(&reg.key).unwrap().request, reg.request);
            assert!(super::super::commit(reg, &json!({"user_input":"blocked"}), true).is_err());
            assert!(!reg.result_path().unwrap().exists());
        }
        assert!(guarded_publications(&config).is_err());
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let client = Client { config: config.clone(), token: "isolated-test-token".into(),
                session: "isolated-test-session".into(), http: reqwest::Client::new(), progress: Progress::new() };
            client.control(&json!({"frozen":false,"drain":false,"epoch":1})).await.unwrap();
        });
        let publications = guarded_publications(&config).unwrap();
        assert_eq!(publications.len(), 33);
        for reg in &pending {
            assert!(publications.iter().any(|binding| binding.registration_key == reg.key));
            assert!(super::super::load_registration(&reg.key).unwrap().published);
        }
        println!("real control writer waited for in-flight registration; 32 frozen concurrent requests retained and denied replies/publication; same 32 keys promoted after real control unfreeze");
        match previous {Some(value)=>std::env::set_var("ITERATE_CROSS_DEVICE_DIR",value),None=>std::env::remove_var("ITERATE_CROSS_DEVICE_DIR")}
        match previous_mirror {Some(value)=>std::env::set_var("ITERATE_CROSS_DEVICE_MIRROR",value),None=>std::env::remove_var("ITERATE_CROSS_DEVICE_MIRROR")}
    }

    #[test]
    fn cold_tracking_survives_credential_refresh_while_waiting_for_route() {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        let previous_mirror = std::env::var_os("ITERATE_CROSS_DEVICE_MIRROR");
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR", dir.path());
        std::env::remove_var("ITERATE_CROSS_DEVICE_MIRROR");
        let old = HubConfig { endpoint: "http://127.0.0.1:18555".into(),
            device_id: "refresh-source".into(), token_env: "TEST_OLD_TOKEN".into(), ca_certificate: None };
        atomic_json(&dir.path().join("settings.json"), &super::super::Settings {
            device_id: old.device_id.clone(), enabled: true,
        }).unwrap();
        for (updated, should_retain) in [
            (HubConfig { token_env: "TEST_NEW_TOKEN".into(), ..old.clone() }, true),
            (HubConfig { ca_certificate: Some(dir.path().join("new-ca.pem")), ..old.clone() }, true),
            (HubConfig { device_id: "different-source".into(), ..old.clone() }, false),
            (HubConfig { endpoint: "http://127.0.0.1:18556".into(), ..old.clone() }, false),
        ] {
        atomic_json(&dir.path().join("hub-connection.json"), &old).unwrap();
        let route = super::super::lock(dir.path(), "connection.lock").unwrap();
        let request_file = dir.path().join("request.json");
        let response_file = dir.path().join("response.json");
        let request = json!({"id":"credential-refresh-pending","message":"original pending text"});
        atomic_json(&request_file, &request).unwrap();
        let original_request = request.clone();
        let worker = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
                .block_on(super::super::register(&request, &request_file, &response_file))
        });
        let probe = gate_file().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while probe.try_lock().is_ok() {
            probe.unlock().unwrap();
            assert!(std::time::Instant::now() < deadline, "registration did not reach route lock");
            std::thread::sleep(Duration::from_millis(5));
        }
        // configure_from_stdin permits this change under the same route lock.
        atomic_json(&dir.path().join("hub-connection.json"), &updated).unwrap();
        drop(route);
        let registration = worker.join().unwrap();
        if should_retain {
            let registration = registration.expect("same source/endpoint credential refresh must not lose pending popup registration");
            assert!(!registration.published);
            assert_eq!(registration.request, original_request);
            assert_eq!(registration.origin_device_id, old.device_id);
            assert!(registration.path().unwrap().exists());
            println!("same source/endpoint credential refresh retained original deferred request under route lock");
        } else {
            assert!(registration.is_none(), "source or endpoint migration must reject stale registration");
            println!("source/endpoint route change rejected stale registration");
        }
        }
        match previous {Some(value)=>std::env::set_var("ITERATE_CROSS_DEVICE_DIR",value),None=>std::env::remove_var("ITERATE_CROSS_DEVICE_DIR")}
        match previous_mirror {Some(value)=>std::env::set_var("ITERATE_CROSS_DEVICE_MIRROR",value),None=>std::env::remove_var("ITERATE_CROSS_DEVICE_MIRROR")}
    }

    #[test]
    fn cloud_mac_unsupported_deferred_windows_do_not_block_text_publications() {
        let dir=tempfile::tempdir().unwrap();
        let previous=std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR",dir.path());
        let config=HubConfig{endpoint:"http://127.0.0.1:18555".into(),device_id:"mac-source".into(),token_env:"TEST_HUB_TOKEN".into(),ca_certificate:None};
        atomic_json(&dir.path().join("hub-connection.json"),&config).unwrap();
        atomic_json(&dir.path().join("settings.json"),&super::super::Settings{device_id:config.device_id.clone(),enabled:true}).unwrap();
        atomic_json(&dir.path().join("hub-control.json"),&json!({"frozen":false,"drain":false})).unwrap();
        let make_request=|request:Value,published:bool| {
            let key=uuid::Uuid::new_v4().to_string();
            let request_file=dir.path().join(format!("{key}-request.json"));
            atomic_json(&request_file,&request).unwrap();
            let reg=super::super::Registration{key,origin_device_id:config.device_id.clone(),origin_name:"Mac".into(),origin_platform:Some("macos".into()),request,request_file,response_file:dir.path().join(format!("{}-response.json",uuid::Uuid::new_v4())),published};
            atomic_json(&reg.path().unwrap(),&reg).unwrap();reg.renew();reg
        };
        let normal=make_request(json!({"id":"normal-published","message":"normal text"}),true);
        let deferred=make_request(json!({"id":"normal-deferred","message":"pending text"}),false);
        let unsupported=[
            make_request(json!({"id":"attachment","message":"local image","images":["private-local-image"]}),false),
            make_request(json!({"id":"markdown","message":"![local](private.png)"}),false),
            make_request(json!({"id":"html","message":"<img src='private.png'>"}),false),
        ];
        for _ in 0..2 {
            let _admission=submission_guard(false).unwrap();
            let _route=super::super::lock(dir.path(),"connection.lock").unwrap();
            // Exercise the production Mac branch even on a Windows test host.
            let publications=registration_publications_for_platform(&config,true).unwrap();
            let keys:std::collections::HashSet<_>=publications.iter().map(|b|b.registration_key.clone()).collect();
            assert_eq!(keys,std::collections::HashSet::from([normal.key.clone(),deferred.key.clone()]));
            assert!(publications.iter().all(|b|b.kind=="mirror" && b.route_id.starts_with("cross-mirror:")));
            for reg in &unsupported {
                assert!(!super::super::load_registration(&reg.key).unwrap().published);
                assert!(reg.path().unwrap().exists());
                assert!(!binding_path(&reg.key).unwrap().exists());
                assert!(!reg.response_file.exists());
                let rejection:Value=read_json(&dir.path().join("hub-publication-rejections").join(format!("{}.json",digest(&reg.key)))).unwrap();
                assert_eq!(rejection["reason"],"mirror_text_only");
                assert_eq!(rejection.as_object().unwrap().len(),3);
            }
        }
        assert!(super::super::load_registration(&deferred.key).unwrap().published);
        match previous {Some(value)=>std::env::set_var("ITERATE_CROSS_DEVICE_DIR",value),None=>std::env::remove_var("ITERATE_CROSS_DEVICE_DIR")}
    }

    #[test]
    fn cloud_deferred_publication_preserves_source_and_all_admission_boundaries() {
        let dir=tempfile::tempdir().unwrap();
        let previous=std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR",dir.path());
        let config=HubConfig{endpoint:"http://127.0.0.1:18555".into(),device_id:"configured-cloud-source".into(),
            token_env:"TEST_HUB_TOKEN".into(),ca_certificate:None};
        let local=super::super::Settings{device_id:"original-local-source".into(),enabled:true};
        atomic_json(&dir.path().join("hub-connection.json"),&config).unwrap();
        atomic_json(&dir.path().join("settings.json"),&local).unwrap();
        atomic_json(&dir.path().join("hub-control.json"),&json!({"frozen":false,"drain":false})).unwrap();
        let make_request=|| {
            let key=uuid::Uuid::new_v4().to_string();
            let request_file=dir.path().join(format!("{key}-request.json"));
            let request=json!({"id":format!("original-{key}"),"message":"pending Mac source",
                "project_path":"/original/project","predefined_options":["keep original"]});
            atomic_json(&request_file,&request).unwrap();
            let reg=super::super::Registration{key,origin_device_id:local.device_id.clone(),origin_name:"original Mac".into(),
                origin_platform:Some("macos".into()),request,request_file,response_file:dir.path().join(format!("{}-response.json",uuid::Uuid::new_v4())),published:false};
            atomic_json(&reg.path().unwrap(),&reg).unwrap();reg.renew();reg
        };
        let pending=make_request();
        let response=make_request();atomic_json(&response.response_file,&json!({"user_input":"answered"})).unwrap();
        let completed=make_request();atomic_json(&completed.result_path().unwrap(),&json!({"cancelled":true})).unwrap();
        let stale=make_request();fs::remove_file(dir.path().join("leases").join(&stale.key)).unwrap();
        let missing=make_request();fs::remove_file(&missing.request_file).unwrap();
        atomic_json(&dir.path().join("settings.json"),&super::super::Settings{enabled:false,..local.clone()}).unwrap();
        assert!(guarded_publications(&config).unwrap().is_empty());
        assert!(!super::super::load_registration(&pending.key).unwrap().published);
        atomic_json(&dir.path().join("settings.json"),&local).unwrap();
        for drain in [false,true] {
            atomic_json(&dir.path().join("hub-control.json"),&json!({"frozen":true,"drain":drain})).unwrap();
            assert!(guarded_publications(&config).is_err());
            assert!(!super::super::load_registration(&pending.key).unwrap().published);
        }
        atomic_json(&dir.path().join("hub-control.json"),&json!({"frozen":false,"drain":false})).unwrap();
        let changed=HubConfig{device_id:"another-source".into(),..config.clone()};
        assert!(guarded_publications(&changed).is_err());
        assert!(!super::super::load_registration(&pending.key).unwrap().published);
        let selected=guarded_publications(&config).unwrap();
        assert_eq!(selected.len(),1);assert_eq!(selected[0].registration_key,pending.key);
        assert_eq!(selected[0].source_id,config.device_id);
        let promoted=super::super::load_registration(&pending.key).unwrap();
        assert!(promoted.published);assert!(!pending.path().unwrap().exists());
        assert_eq!(promoted.origin_device_id,pending.origin_device_id);
        assert_eq!(promoted.request,pending.request);assert_eq!(promoted.request_file,pending.request_file);
        assert_eq!(promoted.response_file,pending.response_file);assert!(!promoted.result_path().unwrap().exists());
        let repeated=guarded_publications(&config).unwrap();
        assert_eq!(repeated[0].registration_key,selected[0].registration_key);
        assert_eq!(repeated[0].route_id,selected[0].route_id);assert_eq!(repeated[0].version,selected[0].version);
        for excluded in [&response,&completed,&stale,&missing] {
            assert!(!super::super::load_registration(&excluded.key).unwrap().published);
            assert!(!binding_path(&excluded.key).unwrap().exists());
        }
        // Never overwrite a persisted binding belonging to another cloud source.
        let conflict=make_request();
        let mut published=conflict.clone();published.published=true;
        let mut binding=registration_binding(&config,&published).unwrap();binding.source_id="different-cloud-source".into();
        atomic_json(&binding_path(&conflict.key).unwrap(),&binding).unwrap();
        assert!(guarded_publications(&config).is_err());
        assert!(!super::super::load_registration(&conflict.key).unwrap().published);
        assert_eq!(read_json::<Binding>(&binding_path(&conflict.key).unwrap()).unwrap().source_id,binding.source_id);
        match previous {
            Some(value)=>std::env::set_var("ITERATE_CROSS_DEVICE_DIR",value),
            None=>std::env::remove_var("ITERATE_CROSS_DEVICE_DIR"),
        }
    }

    #[test]
    fn cloud_mac_publish_wire_keeps_legacy_envelope_and_bounded_request() {
        let route=format!("cross-mirror:{}","a".repeat(64));
        let wire=mirror_wire(&route,"fixture-mac","Mac",&json!({"message":"published text","predefined_options":["one"],"is_markdown":true,"source":"must-not-copy","project_path":"/private/source"}));
        assert_eq!(wire["message_type"],"cross_device_mirror_state");
        let p=&wire["payload"];assert_eq!(p["source"],"cross_device_mirror");
        assert_eq!(p["request_id"],route);assert_eq!(p["mirror_id"],route);
        assert_eq!(p["project_path"],"");assert_eq!(p["request"]["project_path"],"");
        assert_eq!(p["request"]["id"],route);assert_eq!(p["request"]["message"],"published text");
        assert!(p["request"].as_object().unwrap().keys().all(|k|matches!(k.as_str(),"id"|"message"|"predefined_options"|"is_markdown"|"project_path")));
        assert!(p["request"].get("source").is_none());
    }
    #[test]
    fn endpoint_validation_never_disables_tls_identity() {
        let mut c = HubConfig { endpoint:"https://8.129.82.226:8443".into(), device_id:"win".into(), token_env:"ITERATE_TEST_TOKEN".into(), ca_certificate:None };
        assert!(validate(&c).is_ok());
        for invalid in ["http://8.129.82.226:8443", "https://user:secret@example.com", "https://example.com/?token=x"] {
            c.endpoint=invalid.into(); assert!(validate(&c).is_err());
        }
        c.endpoint="http://127.0.0.1:18555".into(); assert!(validate(&c).is_ok());
    }
    #[test]
    fn cloud_source_original_winner_and_freeze_share_product_lock() {
        let dir=tempfile::tempdir().unwrap();std::env::set_var("ITERATE_CROSS_DEVICE_DIR",dir.path());
        let c=HubConfig{endpoint:"http://127.0.0.1:18555".into(),device_id:"source-fixture".into(),token_env:"TEST_HUB_TOKEN".into(),ca_certificate:None};
        atomic_json(&dir.path().join("hub-connection.json"),&c).unwrap();
        atomic_json(&dir.path().join("hub-control.json"),&json!({"frozen":false,"drain":false})).unwrap();
        let request_file=dir.path().join("request.json");atomic_json(&request_file,&json!({})).unwrap();
        let reg=super::super::Registration{key:uuid::Uuid::new_v4().to_string(),origin_device_id:c.device_id.clone(),origin_name:"fixture".into(),origin_platform:Some(std::env::consts::OS.into()),
            request:json!({"id":"fixture-request","message":"isolated fixture","project_path":"C:/isolated"}),request_file,response_file:dir.path().join("response.json"),published:true};
        atomic_json(&reg.path().unwrap(),&reg).unwrap();reg.renew();
        let binding=registration_binding(&c,&reg).unwrap();assert_eq!(registration_binding(&c,&reg).unwrap().registration_key,binding.registration_key);
        let response=json!({"user_input":"exact original","metadata":{"source":"unchanged"}});
        {let _cloud=submission_guard(true).unwrap();super::super::commit_inner(&reg,&response,true).unwrap();}
        super::super::commit(&reg,&response,false).unwrap();
        assert!(super::super::commit(&reg,&json!({"user_input":"competing"}),false).is_err());
        assert_eq!(read_json::<Value>(&reg.result_path().unwrap()).unwrap()["response"],response);
        let connection=legacy_connection().unwrap();assert!(!legacy_connections_closed().unwrap());drop(connection);assert!(legacy_connections_closed().unwrap());
        fs::create_dir_all(dir.path().join("finalizing")).unwrap();
        let finalizer=OpenOptions::new().create(true).read(true).write(true).open(dir.path().join("finalizing/fixture")).unwrap();
        finalizer.lock().unwrap();assert!(!finalizations_quiescent().unwrap());drop(finalizer);assert!(finalizations_quiescent().unwrap());
        atomic_json(&dir.path().join("hub-control.json"),&json!({"frozen":true,"drain":false})).unwrap();
        assert!(submission_guard(false).is_err());assert!(submission_guard(true).is_err());assert!(legacy_connection().is_err());
        let runtime=tokio::runtime::Runtime::new().unwrap();
        assert!(runtime.block_on(super::super::submit(&json!({"user_input":"unregistered"}))).is_err());
        atomic_json(&dir.path().join("hub-control.json"),&json!({"frozen":true,"drain":true})).unwrap();
        assert!(submission_guard(false).is_err());assert!(submission_guard(true).is_ok());
        std::env::remove_var("ITERATE_CROSS_DEVICE_DIR");
    }
}
