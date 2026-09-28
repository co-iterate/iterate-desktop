//! iOS authorization generations. The legacy paired-device/APNs files are never
//! rewritten by this store. Android keeps its existing store and recovery policy.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
struct PushRecord<T> {
    generation: String,
    info: T,
}

#[derive(Default, Serialize, Deserialize)]
struct IosStore {
    version: u32,
    legacy_import_status: String,
    devices: Vec<PairedDeviceRecord>,
    consumed_claims: Vec<String>,
    #[serde(default)]
    new_generations: Vec<String>,
    notifications: HashMap<String, PushRecord<ApnsDeviceInfo>>,
    activities: HashMap<String, PushRecord<ApnsLiveActivityInfo>>,
}

fn state_path(paired_path: &FilePath) -> PathBuf {
    paired_path.with_file_name("ios-generations-v1.json")
}

// A separate monotonic reservation index keeps Android claims independent of
// damage to the iOS credential/push state. It contains IDs, never credentials.
#[derive(Serialize, Deserialize)]
struct ReservedIds { ids: Vec<String>, sha256: String }

fn reserved_path(paired_path: &FilePath) -> PathBuf {
    paired_path.with_file_name("ios-reserved-ids-v1.json")
}

fn read_reserved_ids(path: &FilePath) -> Result<Option<Vec<String>>, String> {
    let bytes = match std::fs::read(reserved_path(path)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("ios_reserved_ids_unavailable".into()),
    };
    let index: ReservedIds = serde_json::from_slice(&bytes).map_err(|_| "ios_reserved_ids_invalid")?;
    let canonical = serde_json::to_string(&index.ids).map_err(|e| e.to_string())?;
    if bridge_token_hash(&canonical) != index.sha256 || index.ids.windows(2).any(|ids| ids[0] >= ids[1]) {
        return Err("ios_reserved_ids_invalid".into());
    }
    Ok(Some(index.ids))
}

fn reserve_store_ids(path: &FilePath, store: &IosStore) -> Result<(), String> {
    let previous = read_reserved_ids(path)?;
    let mut ids = previous.clone().unwrap_or_default();
    ids.extend(store.devices.iter().map(|d| d.device_id.clone()));
    ids.sort(); ids.dedup();
    if previous.as_ref() != Some(&ids) {
        let sha256 = bridge_token_hash(&serde_json::to_string(&ids).map_err(|e| e.to_string())?);
        atomic_write_private_file(&reserved_path(path), &serde_json::to_vec(&ReservedIds { ids, sha256 }).map_err(|e| e.to_string())?)?;
    }
    Ok(())
}

fn with_store<T>(paired_path: &FilePath, action: impl FnOnce(&mut IosStore) -> Result<(T, bool), String>) -> Result<T, String> {
    let path = state_path(paired_path);
    let lock_path = path.with_extension("lock");
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let lock = OpenOptions::new().read(true).write(true).create(true).open(lock_path).map_err(|e| e.to_string())?;
    lock_private_state_file(&lock)?;
    // A durable existence marker prevents a missing state file from importing
    // old credentials again after an interrupted restore/deletion.
    let marker = path.with_extension("initialized");
    let (mut store, initialized) = match std::fs::read(&path) {
        Ok(bytes) => {
            let value: IosStore = serde_json::from_slice(&bytes).map_err(|_| "ios_generation_store_invalid")?;
            if value.version != 1 { return Err("ios_generation_store_version".into()); }
            (value, false)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !marker.exists() => {
            // Import only the primary file, never a possibly older backup. A
            // damaged primary quarantines legacy iOS without changing Android.
            let (devices, status) = match read_paired_device_store_candidate(paired_path) {
                Ok(Some((value, _))) => (value.devices.into_iter().filter(|d| d.client_kind.eq_ignore_ascii_case("ios")).collect(), "primary"),
                Ok(None) => (Vec::new(), "absent"),
                Err(_) => (Vec::new(), "quarantined_invalid_primary"),
            };
            (IosStore { version: 1, devices, legacy_import_status: status.into(), ..Default::default() }, true)
        }
        Err(_) => return Err("ios_generation_store_unavailable".into()),
    };
    let (result, changed) = action(&mut store)?;
    // Reserve before committing a credential. A crash may leave an extra ID
    // reserved, but can never issue a generation whose ID is absent here.
    reserve_store_ids(paired_path, &store)?;
    if initialized || changed {
        let bytes = serde_json::to_vec_pretty(&store).map_err(|e| e.to_string())?;
        atomic_write_private_file(&path, &bytes)?;
    }
    if !marker.exists() { atomic_write_private_file(&marker, b"ios-generations-v1\n")?; }
    Ok(result)
}

pub(super) fn initialize_at(path: &FilePath) -> Result<(), String> {
    with_store(path, |_| Ok(((), false)))
}

fn principal_id(record: &PairedDeviceRecord) -> String {
    format!("ios:{}:{}", record.device_id, record.token_hash)
}

pub(super) fn management_generation(record: &PairedDeviceRecord) -> String {
    bridge_token_hash(&format!("desktop-file-roots:{}", principal_id(record)))
}

fn active<'a>(store: &'a IosStore, id: &str) -> Option<&'a PairedDeviceRecord> {
    let mut rows = store.devices.iter().filter(|d| d.device_id == id && d.revoked_at.is_none());
    let first = rows.next()?;
    rows.next().is_none().then_some(first)
}

pub(super) fn authenticate_at(path: &FilePath, token: &str, requested_id: Option<&str>) -> Result<Option<AuthPrincipal>, String> {
    with_store(path, |store| {
        let found = store.devices.iter().find(|d| requested_id.is_none_or(|id| id == d.device_id) && bridge_token_hash_matches(token, &d.token_hash));
        let principal = found.filter(|d| active(store, &d.device_id).is_some_and(|current| current.token_hash == d.token_hash)).map(|d| AuthPrincipal {
            principal_id: principal_id(d), device_id: d.device_id.clone(), client_kind: "ios".into(), scopes: d.scopes.iter().filter(|scope|ios_device_scopes(true).contains(scope)).cloned().collect(),
        });
        Ok((principal, false))
    })
}

pub(super) fn id_reserved_at(path: &FilePath, id: &str) -> Result<bool, String> {
    let ids = match read_reserved_ids(path)? {
        Some(ids) => ids,
        None => { initialize_at(path)?; read_reserved_ids(path)?.ok_or("ios_reserved_ids_unavailable")? }
    };
    Ok(ids.iter().any(|reserved| reserved == id))
}

pub(super) fn generation_is_active(id: &str, generation: &str) -> bool {
    with_store(&paired_devices_path(), |store| Ok((active(store,id).is_some_and(|d|principal_id(d)==generation),false))).unwrap_or(false)
}

/// Caller holds the shared cross-process paired-device lock. This transaction
/// consumes the QR and revokes/appends a generation in one atomic private file.
pub(super) fn claim_at(path: &FilePath, old_store: &PairedDeviceStore, record: PairedDeviceRecord, claim_hash: &str) -> Result<(), String> {
    if old_store.devices.iter().any(|d| d.device_id == record.device_id && !d.client_kind.eq_ignore_ascii_case("ios")) {
        return Err("device_id_android_collision".into());
    }
    with_store(path, |store| {
        if store.consumed_claims.iter().any(|h| h == claim_hash) { return Err("pairing_token_already_claimed".into()); }
        for old in store.devices.iter_mut().filter(|d| d.device_id == record.device_id && d.revoked_at.is_none()) {
            old.revoked_at = Some(record.created_at.clone());
        }
        assert!(record.file_browser_roots.is_empty());
        store.devices.push(record);
        store.new_generations.push(store.devices.last().unwrap().token_hash.clone());
        store.consumed_claims.push(claim_hash.into());
        Ok(((), true))
    })
}

fn authorized_push(store: &IosStore, principal: &AuthPrincipal) -> bool {
    active(store, &principal.device_id).is_some_and(|d| principal_id(d) == principal.principal_id && store.new_generations.contains(&d.token_hash))
}

fn live_generation(store: &IosStore, device_id: &str, generation: &str) -> bool {
    active(store, device_id).is_some_and(|d| principal_id(d) == generation && store.new_generations.contains(&d.token_hash))
}

pub(super) fn legacy_known_roots(principal: &AuthPrincipal) -> bool {
    with_store(&paired_devices_path(), |store| Ok((active(store, &principal.device_id).is_some_and(|d| principal_id(d)==principal.principal_id && !store.new_generations.contains(&d.token_hash)), false))).unwrap_or(false)
}

fn push_error(error: String) -> std::io::Error { std::io::Error::other(error) }

pub(super) async fn register_apns_device_token(token: String, mut info: ApnsDeviceInfo, principal: &AuthPrincipal) -> Result<(), std::io::Error> {
    with_store(&paired_devices_path(), |store| {
        if !authorized_push(store, principal) { return Err("active_new_ios_generation_required".into()); }
        info.device_id=principal.device_id.clone();
        store.notifications.retain(|_, r| r.generation != principal.principal_id || r.info.environment != info.environment);
        store.notifications.insert(token, PushRecord { generation: principal.principal_id.clone(), info });
        Ok(((), true))
    }).map_err(push_error)
}

pub(super) async fn register_apns_live_activity_token(token: String, mut info: ApnsLiveActivityInfo, kind: &str, key: &str, principal: &AuthPrincipal) -> (usize, Result<(), std::io::Error>) {
    let result=with_store(&paired_devices_path(), |store| {
        if !authorized_push(store, principal) { return Err("active_new_ios_generation_required".into()); }
        info.device_id=principal.device_id.clone();
        store.activities.retain(|_, r| r.generation != principal.principal_id || !live_activity_info_matches(&r.info,kind,key) || r.info.environment != info.environment);
        store.activities.insert(token, PushRecord { generation: principal.principal_id.clone(), info });
        Ok((store.activities.len(), true))
    });
    match result { Ok(n)=>(n,Ok(())),Err(e)=>(0,Err(push_error(e))) }
}

pub(super) async fn apns_device_tokens_snapshot() -> HashMap<String, ApnsDeviceInfo> {
    with_store(&paired_devices_path(), |store| Ok((store.notifications.iter().filter(|(_,r)| live_generation(store,&r.info.device_id,&r.generation) && !super::super::apns_notification::is_apns_token_stale(&r.info)).map(|(k,r)| { let mut info=r.info.clone();info.authorization_generation=r.generation.clone();(k.clone(),info) }).collect(), false))).unwrap_or_default()
}

pub(super) async fn apns_live_activity_tokens_snapshot() -> HashMap<String, ApnsLiveActivityInfo> {
    with_store(&paired_devices_path(), |store| Ok((store.activities.iter().filter(|(_,r)| live_generation(store,&r.info.device_id,&r.generation) && !super::super::apns_live_activity::is_apns_live_activity_token_stale(&r.info)).map(|(k,r)| { let mut info=r.info.clone();info.authorization_generation=r.generation.clone();(k.clone(),info) }).collect(), false))).unwrap_or_default()
}

pub(super) async fn apns_device_token_count() -> usize { apns_device_tokens_snapshot().await.len() }
fn remove_apns_device_tokens_at(path: &FilePath, tokens: &[(String, String)]) -> Result<(), String> {
    with_store(path, |store| {
        let mut changed = false;
        for (token, generation) in tokens {
            if store.notifications.get(token).is_some_and(|row| row.generation == *generation) {
                store.notifications.remove(token);
                changed = true;
            }
        }
        Ok(((), changed))
    })
}
pub(super) async fn remove_apns_device_tokens(tokens: &[(String, String)]) -> Result<(),std::io::Error> {
    remove_apns_device_tokens_at(&paired_devices_path(), tokens).map_err(push_error)
}
fn remove_apns_live_activity_tokens_at(path: &FilePath, tokens: &[(String, String)]) -> Result<(), String> {
    with_store(path, |store| {
        let mut changed = false;
        for (token, generation) in tokens {
            if store.activities.get(token).is_some_and(|row| row.generation == *generation) {
                store.activities.remove(token);
                changed = true;
            }
        }
        Ok(((), changed))
    })
}
pub(super) async fn remove_apns_live_activity_tokens(tokens: &[(String, String)]) -> Result<(),std::io::Error> {
    remove_apns_live_activity_tokens_at(&paired_devices_path(), tokens).map_err(push_error)
}
pub(super) async fn update_apns_device_notification_preference(token: &str, principal: &AuthPrincipal, enabled: bool, seen: &str) -> Result<ApnsNotificationPreferenceUpdate,std::io::Error> {
    with_store(&paired_devices_path(),|store| {
        if !authorized_push(store,principal) { return Err("active_new_ios_generation_required".into()); }
        let Some(row)=store.notifications.get_mut(token) else { return Ok((ApnsNotificationPreferenceUpdate::TokenNotFound,false)); };
        if row.generation!=principal.principal_id { return Ok((ApnsNotificationPreferenceUpdate::DeviceMismatch,false)); }
        row.info.notifications_enabled=enabled;row.info.last_seen_at=seen.into();
        Ok((ApnsNotificationPreferenceUpdate::Updated,true))
    }).map_err(push_error)
}

pub(super) fn push_token_is_current(token: &str, activity: bool, generation: &str) -> bool {
    with_store(&paired_devices_path(),|store| Ok((if activity {
        store.activities.get(token).is_some_and(|r| r.generation == generation && live_generation(store,&r.info.device_id,&r.generation))
    } else { store.notifications.get(token).is_some_and(|r| r.generation == generation && live_generation(store,&r.info.device_id,&r.generation)) },false))).unwrap_or(false)
}

pub(super) fn active_records_at(path: &FilePath) -> Result<Vec<PairedDeviceRecord>, String> {
    with_store(path, |store| Ok((store.devices.iter().filter(|d| active(store, &d.device_id).is_some_and(|a| a.token_hash == d.token_hash)).cloned().collect(), false)))
}

pub(super) fn roots_at(path: &FilePath, principal: &AuthPrincipal) -> Vec<PathBuf> {
    with_store(path, |store| Ok((active(store, &principal.device_id).filter(|d| principal_id(d) == principal.principal_id).map(|d| d.file_browser_roots.iter().map(PathBuf::from).collect()).unwrap_or_default(), false))).unwrap_or_default()
}

pub(super) fn update_roots_at(path: &FilePath, device_id: &str, expected_generation: &str, roots: Vec<String>) -> Result<bool, String> {
    with_store(path, |store| {
        let Some(generation) = active(store, device_id).map(|d| d.token_hash.clone()) else { return Ok((false, false)); };
        let row = store.devices.iter_mut().find(|d| d.device_id == device_id && d.token_hash == generation).unwrap();
        if management_generation(row) != expected_generation {
            return Err("ios_file_roots_generation_mismatch".into());
        }
        row.file_browser_roots = roots;
        Ok((true, true))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(id: &str, token: &str) -> PairedDeviceRecord {
        PairedDeviceRecord { device_id:id.into(),device_name:"iPhone".into(),client_kind:"ios".into(),token_hash:bridge_token_hash(token),scopes:ios_device_scopes(false),created_at:"2026-09-27T00:00:00Z".into(),last_seen_at:"2026-09-27T00:00:00Z".into(),file_browser_roots:Vec::new(),revoked_at:None }
    }
    fn fixture() -> (tempfile::TempDir,PathBuf) {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("paired-devices.json");(dir,path)
    }
    #[test]
    fn old_valid_credentials_survive_but_revocations_and_audit_are_immutable() {
        let (_dir,path)=fixture();let valid=device("valid","old-valid");let mut revoked=device("revoked","old-revoked");revoked.revoked_at=Some("2026-09-20T00:00:00Z".into());
        save_paired_device_store_at(&path,&PairedDeviceStore{devices:vec![valid,revoked]}).unwrap();let before=std::fs::read(&path).unwrap();
        assert!(authenticate_at(&path,"old-valid",Some("valid")).unwrap().is_some());
        assert!(authenticate_at(&path,"old-revoked",Some("revoked")).unwrap().is_none());
        assert_eq!(before,std::fs::read(&path).unwrap());
        with_store(&path,|store|{assert_eq!(store.devices.len(),2);assert!(!store.devices[1].revoked_at.as_ref().unwrap().is_empty());assert!(store.notifications.is_empty()&&store.activities.is_empty());Ok(((),false))}).unwrap();
    }
    #[test]
    fn same_id_rotation_consumes_once_preserves_tombstone_and_resets_roots() {
        let (_dir,path)=fixture();let mut old=device("iphone","old");old.file_browser_roots.push("old-root".into());
        let source=PairedDeviceStore{devices:vec![old]};save_paired_device_store_at(&path,&source).unwrap();
        let old_principal=authenticate_at(&path,"old",Some("iphone")).unwrap().unwrap();
        claim_at(&path,&source,device("iphone","new"),"qr-once").unwrap();
        assert!(claim_at(&path,&source,device("iphone","replay"),"qr-once").is_err());
        assert!(authenticate_at(&path,"old",Some("iphone")).unwrap().is_none());
        let new_principal=authenticate_at(&path,"new",Some("iphone")).unwrap().unwrap();assert_ne!(old_principal.principal_id,new_principal.principal_id);
        assert!(roots_at(&path,&old_principal).is_empty());assert!(roots_at(&path,&new_principal).is_empty());
        with_store(&path,|store|{assert_eq!(store.devices.len(),2);assert!(store.devices[0].revoked_at.is_some());assert_eq!(store.devices[0].file_browser_roots,vec!["old-root"]);assert_eq!(store.consumed_claims,vec!["qr-once"]);assert!(authorized_push(store,&new_principal));assert!(!authorized_push(store,&old_principal));Ok(((),false))}).unwrap();
    }
    #[test]
    fn android_collision_does_not_consume_ios_claim() {
        let (_dir,path)=fixture();let mut android=device("shared","android");android.client_kind="android".into();android.scopes=mobile_device_scopes(false);
        let source=PairedDeviceStore{devices:vec![android]};save_paired_device_store_at(&path,&source).unwrap();
        assert_eq!(claim_at(&path,&source,device("shared","ios"),"qr").unwrap_err(),"device_id_android_collision");
        claim_at(&path,&source,device("unique","ios"),"qr").unwrap();assert!(id_reserved_at(&path,"unique").unwrap());
    }
    #[test]
    fn push_authorization_requires_new_registration_and_effective_generation() {
        let (_dir,path)=fixture();let source=PairedDeviceStore{devices:vec![device("iphone","legacy")]};
        save_paired_device_store_at(&path,&source).unwrap();
        let legacy=authenticate_at(&path,"legacy",None).unwrap().unwrap();
        with_store(&path,|store|{assert!(!authorized_push(store,&legacy));assert!(store.notifications.is_empty());Ok(((),false))}).unwrap();
        claim_at(&path,&source,device("iphone","first"),"first-qr").unwrap();
        let first=authenticate_at(&path,"first",None).unwrap().unwrap();
        with_store(&path,|store|{
            assert!(authorized_push(store,&first));assert!(!authorized_push(store,&legacy));
            let info:ApnsDeviceInfo=serde_json::from_value(serde_json::json!({"device_token":"new-registration","platform":"ios","app_version":"1","device_id":"iphone","registered_at":chrono::Utc::now().to_rfc3339()})).unwrap();
            store.notifications.insert("new-registration".into(),PushRecord{generation:first.principal_id.clone(),info});
            assert!(live_generation(store,"iphone",&first.principal_id));Ok(((),true))
        }).unwrap();
        claim_at(&path,&source,device("iphone","second"),"second-qr").unwrap();
        let second=authenticate_at(&path,"second",None).unwrap().unwrap();
        with_store(&path,|store|{
            assert!(!live_generation(store,"iphone",&first.principal_id));assert!(live_generation(store,"iphone",&second.principal_id));
            assert!(!authorized_push(store,&first));
            let old_registration=store.notifications.get("new-registration").unwrap();
            assert!(!live_generation(store,&old_registration.info.device_id,&old_registration.generation));
            assert!(store.activities.get("direct-unregistered-token").is_none());Ok(((),false))
        }).unwrap();
    }
    #[test]
    fn delayed_old_apns_cleanup_does_not_delete_same_token_registered_by_new_generation() {
        let (_dir, path) = fixture();
        let source = PairedDeviceStore::default();
        save_paired_device_store_at(&path, &source).unwrap();
        claim_at(&path, &source, device("iphone", "first"), "first-qr").unwrap();
        let first = authenticate_at(&path, "first", Some("iphone")).unwrap().unwrap();
        claim_at(&path, &source, device("iphone", "second"), "second-qr").unwrap();
        let second = authenticate_at(&path, "second", Some("iphone")).unwrap().unwrap();
        let notification: ApnsDeviceInfo = serde_json::from_value(serde_json::json!({
            "device_token": "same-token", "platform": "ios", "app_version": "1",
            "device_id": "iphone", "registered_at": chrono::Utc::now().to_rfc3339()
        })).unwrap();
        let activity: ApnsLiveActivityInfo = serde_json::from_value(serde_json::json!({
            "activity_token": "same-token", "goal_id": "goal", "device_id": "iphone",
            "registered_at": chrono::Utc::now().to_rfc3339()
        })).unwrap();
        with_store(&path, |store| {
            store.notifications.insert("same-token".into(), PushRecord {
                generation: second.principal_id.clone(), info: notification,
            });
            store.activities.insert("same-token".into(), PushRecord {
                generation: second.principal_id.clone(), info: activity,
            });
            Ok(((), true))
        }).unwrap();

        let stale = [("same-token".to_string(), first.principal_id.clone())];
        remove_apns_device_tokens_at(&path, &stale).unwrap();
        remove_apns_live_activity_tokens_at(&path, &stale).unwrap();
        with_store(&path, |store| {
            assert_eq!(store.notifications["same-token"].generation, second.principal_id);
            assert_eq!(store.activities["same-token"].generation, second.principal_id);
            Ok(((), false))
        }).unwrap();

        let current = [("same-token".to_string(), second.principal_id.clone())];
        remove_apns_device_tokens_at(&path, &current).unwrap();
        remove_apns_live_activity_tokens_at(&path, &current).unwrap();
        with_store(&path, |store| {
            assert!(!store.notifications.contains_key("same-token"));
            assert!(!store.activities.contains_key("same-token"));
            Ok(((), false))
        }).unwrap();
    }
    #[test]
    fn corrupt_ios_store_fails_closed_without_changing_android_backup_recovery() {
        let (_dir,path)=fixture();let mut android=device("android","android-token");android.client_kind="android".into();android.scopes=mobile_device_scopes(false);
        let source=PairedDeviceStore{devices:vec![device("iphone","ios-token"),android]};save_paired_device_store_at(&path,&source).unwrap();save_paired_device_store_at(&path,&source).unwrap();
        std::fs::write(&path,b"broken primary").unwrap();
        let (principal,_,_)=authenticate_paired_device_at(&path,"android-token",Some("android"),chrono::Utc::now()).unwrap();assert_eq!(principal.unwrap().scopes,mobile_device_scopes(false));
        assert!(authenticate_at(&path,"ios-token",Some("iphone")).unwrap().is_none());
        std::fs::write(state_path(&path),b"broken generation store").unwrap();
        assert!(authenticate_at(&path,"ios-token",None).is_err());
        assert!(authenticate_paired_device_at(&path,"android-token",Some("android"),chrono::Utc::now()).unwrap().0.is_some());
        assert!(!id_reserved_at(&path,"new-android").unwrap());
        assert!(!id_reserved_at(&path,"android").unwrap());
    }
    #[test]
    fn damaged_ios_state_only_blocks_reserved_ids_for_new_android_claims() {
        let (_dir,path)=fixture();let source=PairedDeviceStore::default();save_paired_device_store_at(&path,&source).unwrap();
        claim_at(&path,&source,device("ios-only","first"),"qr-first").unwrap();
        claim_at(&path,&source,device("ios-only","second"),"qr-second").unwrap();
        std::fs::write(state_path(&path),b"damaged iOS sidecar").unwrap();
        assert!(id_reserved_at(&path,"ios-only").unwrap());
        let mut android=device("new-android","new-android-token");android.client_kind="android".into();android.scopes=mobile_device_scopes(false);
        mutate_paired_device_store_at(&path,|store| {
            assert!(!id_reserved_at(&path,&android.device_id).unwrap());
            let result=replace_android_paired_device_record(store,android);let changed=result.is_ok();(result,changed)
        }).unwrap().unwrap();
        assert_eq!(authenticate_paired_device_at(&path,"new-android-token",Some("new-android"),chrono::Utc::now()).unwrap().0.unwrap().scopes,mobile_device_scopes(false));
        std::fs::write(reserved_path(&path),b"damaged reservation index").unwrap();
        assert!(id_reserved_at(&path,"unknown").is_err());
    }
    #[test]
    fn generation_state_absence_never_reimports_after_initialization() {
        let (_dir,path)=fixture();save_paired_device_store_at(&path,&PairedDeviceStore{devices:vec![device("iphone","old")]}).unwrap();initialize_at(&path).unwrap();
        std::fs::remove_file(state_path(&path)).unwrap();assert!(authenticate_at(&path,"old",None).is_err());
    }
    #[test]
    fn parallel_claims_have_one_active_generation_and_atomic_consumption() {
        let (_dir,path)=fixture();save_paired_device_store_at(&path,&PairedDeviceStore::default()).unwrap();
        let joins=(0..8).map(|n|{let path=path.clone();std::thread::spawn(move||claim_at(&path,&PairedDeviceStore::default(),device("same",&format!("token-{n}")),"same-qr"))}).collect::<Vec<_>>();
        assert_eq!(joins.into_iter().map(|j|j.join().unwrap().is_ok()).filter(|ok|*ok).count(),1);
        with_store(&path,|store|{assert_eq!(store.devices.len(),1);assert_eq!(store.consumed_claims.len(),1);Ok(((),false))}).unwrap();
    }
    #[test]
    fn ios_generation_process_child() {
        let Ok(directory)=std::env::var("ITERATE_IOS_GENERATION_TEST_DIR") else { return; };
        let ordinal:usize=std::env::var("ITERATE_IOS_GENERATION_TEST_ORDINAL").unwrap().parse().unwrap();
        let root=PathBuf::from(directory);assert!(root.file_name().unwrap().to_string_lossy().starts_with("ios-generation-test-"));
        let path=root.join("paired-devices.json");let result=claim_at(&path,&PairedDeviceStore::default(),device("same",&format!("process-{ordinal}")),"process-qr");
        std::fs::write(root.join(format!("result-{ordinal}")),if result.is_ok(){b"ok".as_slice()}else{b"denied".as_slice()}).unwrap();
    }
    #[test]
    fn cross_process_claim_lock_consumes_exactly_once() {
        let dir=tempfile::Builder::new().prefix("ios-generation-test-").tempdir().unwrap();let path=dir.path().join("paired-devices.json");save_paired_device_store_at(&path,&PairedDeviceStore::default()).unwrap();
        let mut children=Vec::new();
        for n in 0..3 {
            let mut command=std::process::Command::new(std::env::current_exe().unwrap());
            command.arg("ios_generation_process_child").arg("--test-threads=1").env("ITERATE_IOS_GENERATION_TEST_DIR",dir.path()).env("ITERATE_IOS_GENERATION_TEST_ORDINAL",n.to_string()).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            #[cfg(windows)] { use std::os::windows::process::CommandExt;command.creation_flags(0x08000000); }
            children.push(command.spawn().unwrap());
        }
        for mut child in children {assert!(child.wait().unwrap().success());}
        assert_eq!((0..3).filter(|n|std::fs::read(dir.path().join(format!("result-{n}"))).unwrap()==b"ok").count(),1);
        with_store(&path,|store|{assert_eq!(store.devices.len(),1);assert_eq!(store.consumed_claims.len(),1);Ok(((),false))}).unwrap();
    }
}
