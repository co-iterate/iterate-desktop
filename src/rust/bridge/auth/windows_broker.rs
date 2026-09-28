//! A narrowly scoped Windows broker for the Android pairing screen.
//! The DACL and peer checks isolate the current logon session. They do not
//! establish a publisher identity against hostile code running as this user.

use super::*;
use std::{os::windows::io::AsRawHandle, ptr};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, HANDLE},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        GetLengthSid, GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    },
    System::{
        Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
        RemoteDesktop::ProcessIdToSessionId,
        Threading::{
            GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
            QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

const PIPE_PREFIX: &str = r"\\.\pipe\iterate-android-auth-v1";
const IO_TIMEOUT: StdDuration = StdDuration::from_secs(2);

fn current_identity() -> Result<(String, u32), String> {
    let sid = process_sid(unsafe { GetCurrentProcess() })?;
    let mut sid_string = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid.as_ptr().cast_mut().cast(), &mut sid_string) } == 0 {
        return Err("bridge_auth_windows_sid_unavailable".to_string());
    }
    let length = (0..256)
        .position(|index| unsafe { *sid_string.add(index) == 0 })
        .ok_or_else(|| "bridge_auth_windows_sid_invalid".to_string());
    let value = length.map(|length| unsafe {
        String::from_utf16_lossy(std::slice::from_raw_parts(sid_string, length))
    });
    unsafe { LocalFree(sid_string.cast()) };
    let sid_string = value?;
    let session = process_session(unsafe { GetCurrentProcessId() })?;
    Ok((sid_string, session))
}

fn pipe_name() -> Result<String, String> {
    let (sid, session) = current_identity()?;
    #[cfg(test)]
    {
        let test_id = std::env::var("ITERATE_BROKER_TEST_PIPE_ID")
            .ok()
            .filter(|value| !value.is_empty() && value.len() <= 64 && value.bytes().all(|byte| byte.is_ascii_alphanumeric()))
            .unwrap_or_else(|| unsafe { GetCurrentProcessId() }.to_string());
        return Ok(format!("{PIPE_PREFIX}-{sid}-{session}-test-{test_id}"));
    }
    #[cfg(not(test))]
    Ok(format!("{PIPE_PREFIX}-{sid}-{session}"))
}

fn process_sid(process: HANDLE) -> Result<Vec<u8>, String> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return Err("bridge_auth_windows_peer_token_unavailable".to_string());
        }
        let result = (|| {
            let mut size = 0;
            let _ = GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
            if size < std::mem::size_of::<TOKEN_USER>() as u32 || size > 64 * 1024 {
                return Err("bridge_auth_windows_peer_sid_invalid".to_string());
            }
            let mut buffer = vec![0_u8; size as usize];
            if GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            ) == 0
            {
                return Err("bridge_auth_windows_peer_sid_unavailable".to_string());
            }
            let user = ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_USER>());
            let length = GetLengthSid(user.User.Sid) as usize;
            if length == 0 || length > 256 {
                return Err("bridge_auth_windows_peer_sid_invalid".to_string());
            }
            Ok(std::slice::from_raw_parts(user.User.Sid.cast::<u8>(), length).to_vec())
        })();
        CloseHandle(token);
        result
    }
}

fn process_session(pid: u32) -> Result<u32, String> {
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(pid, &mut session) } == 0 {
        return Err("bridge_auth_windows_peer_session_unavailable".to_string());
    }
    Ok(session)
}

fn process_path(process: HANDLE) -> Result<PathBuf, String> {
    let mut buffer = vec![0_u16; 32768];
    let mut size = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size) } == 0 {
        return Err("bridge_auth_windows_peer_path_unavailable".to_string());
    }
    buffer.truncate(size as usize);
    fs::canonicalize(PathBuf::from(String::from_utf16_lossy(&buffer)))
        .map_err(|_| "bridge_auth_windows_peer_path_unavailable".to_string())
}

fn trusted_peer(pid: u32, server_peer: bool) -> Result<(), String> {
    if pid == 0 || process_session(pid)? != process_session(unsafe { GetCurrentProcessId() })? {
        return Err("bridge_auth_windows_peer_session_mismatch".to_string());
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return Err("bridge_auth_windows_peer_unavailable".to_string());
    }
    let result = (|| {
        if process_sid(process)? != process_sid(unsafe { GetCurrentProcess() })? {
            return Err("bridge_auth_windows_peer_sid_mismatch".to_string());
        }
        let peer = process_path(process)?;
        let own = fs::canonicalize(
            std::env::current_exe()
                .map_err(|_| "bridge_auth_windows_process_path_unavailable".to_string())?,
        )
        .map_err(|_| "bridge_auth_windows_process_path_unavailable".to_string())?;
        if peer.parent() != own.parent() {
            return Err("bridge_auth_windows_peer_path_mismatch".to_string());
        }
        let name = peer
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let allowed = if server_peer {
            name.eq_ignore_ascii_case("iterate.exe")
                || (name.starts_with("mobile-bridge-") && name.ends_with(".exe"))
        } else {
            name.eq_ignore_ascii_case("iterate.exe")
                || (name.starts_with("iterate-session-") && name.ends_with(".exe"))
        };
        #[cfg(test)]
        let allowed = allowed || peer == own;
        if !allowed {
            return Err("bridge_auth_windows_peer_path_mismatch".to_string());
        }
        Ok(())
    })();
    unsafe { CloseHandle(process) };
    result
}

fn pairing_route(method: &str, path: &str) -> bool {
    method == "GET"
        && (matches!(
            path,
            "/api/android/pairing"
                | "/api/android/pairing/status"
                | "/api/mobile/pairing"
                | "/api/mobile/pairing/status"
        ) || path_has_one_nonempty_child(path, "/api/android/pairing/sessions/")
            || path_has_one_nonempty_child(path, "/api/mobile/pairing/sessions/"))
}

fn create_server(name: &str, sid: &str, first: bool) -> Result<NamedPipeServer, String> {
    let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err("bridge_auth_windows_acl_invalid".to_string());
    }
    let mut attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let result = unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                name,
                (&mut attrs as *mut SECURITY_ATTRIBUTES).cast(),
            )
    };
    unsafe { LocalFree(descriptor) };
    result.map_err(|error| format!("bridge_auth_windows_pipe_create_failed: {error}"))
}

async fn read_frame<R: AsyncReadExt + Unpin>(stream: &mut R) -> Result<Vec<u8>, String> {
    let mut frame = Vec::new();
    let read = async {
        loop {
            let mut byte = [0_u8; 1];
            if stream
                .read(&mut byte)
                .await
                .map_err(|_| "bridge_auth_windows_pipe_read_failed")?
                == 0
            {
                return Err("bridge_auth_windows_pipe_closed");
            }
            if byte[0] == b'\n' {
                return Ok(frame);
            }
            if frame.len() >= AUTH_BROKER_MAX_MESSAGE_BYTES as usize {
                return Err("bridge_auth_broker_request_too_large");
            }
            frame.push(byte[0]);
        }
    };
    tokio::time::timeout(IO_TIMEOUT, read)
        .await
        .map_err(|_| "bridge_auth_windows_pipe_timeout".to_string())?
        .map_err(str::to_string)
}

async fn serve_client(mut pipe: NamedPipeServer) {
    let mut pid = 0;
    if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle().cast(), &mut pid) } == 0
        || trusted_peer(pid, false).is_err()
    {
        return;
    }
    let response = match read_frame(&mut pipe).await {
        Ok(frame) => match serde_json::from_slice::<AuthBrokerRequest>(&frame) {
            Ok(request)
                if request.version == AUTH_BROKER_PROTOCOL_VERSION
                    && request.audience == BridgeTokenAudience::DesktopRenderer
                    && request.context.is_none()
                    && pairing_route(&request.method, &request.path) =>
            {
                match server_master_key().and_then(|key| {
                    sign_bridge_token_at(
                        key,
                        request.audience,
                        &request.method,
                        &request.path,
                        None,
                        Utc::now(),
                    )
                }) {
                    Ok(token) => AuthBrokerResponse {
                        ok: true,
                        token: Some(token),
                        error: None,
                    },
                    Err(error) => AuthBrokerResponse {
                        ok: false,
                        token: None,
                        error: Some(error),
                    },
                }
            }
            _ => AuthBrokerResponse {
                ok: false,
                token: None,
                error: Some("bridge_auth_broker_request_invalid".to_string()),
            },
        },
        Err(error) => AuthBrokerResponse {
            ok: false,
            token: None,
            error: Some(error),
        },
    };
    if let Ok(mut body) = serde_json::to_vec(&response) {
        body.push(b'\n');
        let _ = tokio::time::timeout(IO_TIMEOUT, pipe.write_all(&body)).await;
    }
}

pub(super) async fn start() -> Result<InternalAuthBroker, String> {
    initialize_server_master_key()?;
    let (sid, _session) = current_identity()?;
    let name = pipe_name()?;
    let mut listener = create_server(&name, &sid, true)?;
    let task = tokio::spawn(async move {
        loop {
            if let Err(error) = listener.connect().await {
                log::error!("[Bridge] Windows 鉴权管道连接失败: {error}");
                break;
            }
            match create_server(&name, &sid, false) {
                Ok(next) => {
                    tokio::spawn(serve_client(listener));
                    listener = next;
                }
                Err(error) => {
                    log::error!("[Bridge] Windows 鉴权管道重建失败: {error}");
                    break;
                }
            }
        }
    });
    Ok(InternalAuthBroker { task })
}

async fn request_async(method: String, path: String) -> Result<String, String> {
    let name = pipe_name()?;
    let mut pipe: NamedPipeClient = ClientOptions::new()
        .open(&name)
        .map_err(|_| "bridge_auth_broker_unavailable".to_string())?;
    let mut pid = 0;
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle().cast(), &mut pid) } == 0 {
        return Err("bridge_auth_windows_server_pid_unavailable".to_string());
    }
    trusted_peer(pid, true)?;
    let request = AuthBrokerRequest {
        version: AUTH_BROKER_PROTOCOL_VERSION,
        audience: BridgeTokenAudience::DesktopRenderer,
        method,
        path,
        context: None,
    };
    let mut body = serde_json::to_vec(&request)
        .map_err(|_| "bridge_auth_broker_request_invalid".to_string())?;
    body.push(b'\n');
    pipe.write_all(&body)
        .await
        .map_err(|_| "bridge_auth_windows_pipe_write_failed".to_string())?;
    let response = serde_json::from_slice::<AuthBrokerResponse>(&read_frame(&mut pipe).await?)
        .map_err(|_| "bridge_auth_broker_response_invalid".to_string())?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "bridge_auth_broker_denied".to_string()));
    }
    response
        .token
        .filter(|token| !token.is_empty() && token.len() <= MAX_TOKEN_BYTES)
        .ok_or_else(|| "bridge_auth_broker_response_invalid".to_string())
}

pub(super) fn request_desktop_token(method: &str, path: &str) -> Result<String, String> {
    let (method, path) = normalize_method_and_path(method, path)?;
    if !pairing_route(&method, &path) {
        return Err("internal_bridge_route_not_allowed".to_string());
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "bridge_auth_windows_runtime_unavailable".to_string())
            .and_then(|runtime| {
                runtime.block_on(async {
                    tokio::time::timeout(IO_TIMEOUT, request_async(method, path))
                        .await
                        .map_err(|_| "bridge_auth_windows_pipe_timeout".to_string())?
                })
            });
        let _ = sender.send(result);
    });
    receiver
        .recv_timeout(IO_TIMEOUT + StdDuration::from_secs(1))
        .map_err(|_| "bridge_auth_windows_pipe_timeout".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_mobile_pairing_get_routes_are_issued() {
        assert!(pairing_route("GET", "/api/android/pairing"));
        assert!(pairing_route("GET", "/api/android/pairing/status"));
        assert!(pairing_route("GET", "/api/android/pairing/sessions/abc"));
        assert!(pairing_route("GET", "/api/mobile/pairing"));
        assert!(pairing_route("GET", "/api/mobile/pairing/status"));
        assert!(pairing_route("GET", "/api/mobile/pairing/sessions/abc"));
        assert!(!pairing_route("POST", "/api/android/pairing"));
        assert!(!pairing_route("POST", "/api/mobile/pairing"));
        assert!(!pairing_route("GET", "/api/config"));
        assert!(!pairing_route("GET", "/api/android/pairing/sessions/a/b"));
        assert!(!pairing_route("GET", "/api/mobile/pairing/sessions/a/b"));
    }

    #[tokio::test]
    async fn private_pipe_round_trip_issues_path_bound_token() {
        let broker = start().await.expect("start private pipe");
        let token =
            tokio::task::spawn_blocking(|| request_desktop_token("GET", "/api/android/pairing"))
                .await
                .expect("join client")
                .expect("broker response");
        assert!(verify_bridge_token_at(
            server_master_key().expect("master key"),
            &token,
            "GET",
            "/api/android/pairing/status",
            None,
            Utc::now(),
        )
        .is_err());
        assert_eq!(
            verify_bridge_token_at(
                server_master_key().expect("master key"),
                &token,
                "GET",
                "/api/android/pairing",
                None,
                Utc::now(),
            )
            .expect("verified token"),
            BridgeTokenAudience::DesktopRenderer
        );
        let ios_token = tokio::task::spawn_blocking(|| {
            request_desktop_token("GET", "/api/mobile/pairing")
        })
        .await
        .expect("join iPhone client")
        .expect("iPhone pairing broker response");
        assert_eq!(
            verify_bridge_token_at(
                server_master_key().expect("master key"),
                &ios_token,
                "GET",
                "/api/mobile/pairing",
                None,
                Utc::now(),
            )
            .expect("verified iPhone pairing token"),
            BridgeTokenAudience::DesktopRenderer
        );
        assert!(verify_bridge_token_at(
            server_master_key().expect("master key"),
            &ios_token,
            "GET",
            "/api/android/pairing",
            None,
            Utc::now(),
        )
        .is_err());
        assert!(request_desktop_token("GET", "/api/config").is_err());

        // A same-session caller can speak the pipe protocol directly, so the
        // server must enforce the narrow route allowlist independently.
        let mut raw = ClientOptions::new().open(pipe_name().unwrap()).unwrap();
        let mut denied = serde_json::to_vec(&AuthBrokerRequest {
            version: AUTH_BROKER_PROTOCOL_VERSION,
            audience: BridgeTokenAudience::DesktopRenderer,
            method: "GET".to_string(),
            path: "/api/config".to_string(),
            context: None,
        })
        .unwrap();
        denied.push(b'\n');
        raw.write_all(&denied).await.unwrap();
        let response: AuthBrokerResponse =
            serde_json::from_slice(&read_frame(&mut raw).await.unwrap()).unwrap();
        assert!(!response.ok);
        assert!(response.token.is_none());
        drop(broker);
    }

    #[test]
    fn broker_child_process() {
        let Ok(role) = std::env::var("ITERATE_BROKER_TEST_ROLE") else {
            return;
        };
        let directory = PathBuf::from(std::env::var("ITERATE_BROKER_TEST_DIR").unwrap());
        match role.as_str() {
            "server" => {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async {
                    let broker = start().await.unwrap();
                    fs::write(directory.join("ready"), std::process::id().to_string()).unwrap();
                    for _ in 0..300 {
                        if directory.join("done").exists() {
                            break;
                        }
                        tokio::time::sleep(StdDuration::from_millis(50)).await;
                    }
                    assert!(directory.join("done").exists(), "client sequence timed out");
                    let result: serde_json::Value =
                        serde_json::from_slice(&fs::read(directory.join("valid.json")).unwrap())
                            .unwrap();
                    let token = result["token"].as_str().unwrap();
                    assert_eq!(
                        verify_bridge_token_at(
                            server_master_key().unwrap(),
                            token,
                            "GET",
                            "/api/mobile/pairing",
                            None,
                            Utc::now(),
                        )
                        .unwrap(),
                        BridgeTokenAudience::DesktopRenderer
                    );
                    assert!(verify_bridge_token_at(
                        server_master_key().unwrap(),
                        token,
                        "GET",
                        "/api/android/pairing",
                        None,
                        Utc::now(),
                    )
                    .is_err());
                    drop(broker);
                });
            }
            "valid" => {
                let token = request_desktop_token("GET", "/api/mobile/pairing").unwrap();
                let result = serde_json::json!({ "pid": std::process::id(), "token": token });
                let temp = directory.join("valid.tmp");
                fs::write(&temp, serde_json::to_vec(&result).unwrap()).unwrap();
                fs::rename(temp, directory.join("valid.json")).unwrap();
            }
            "denied_method" | "denied_path" => {
                let (method, path) = if role == "denied_method" {
                    ("POST", "/api/mobile/pairing")
                } else {
                    ("GET", "/api/config")
                };
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async {
                    let mut pipe = ClientOptions::new().open(pipe_name().unwrap()).unwrap();
                    let request = AuthBrokerRequest {
                        version: AUTH_BROKER_PROTOCOL_VERSION,
                        audience: BridgeTokenAudience::DesktopRenderer,
                        method: method.into(),
                        path: path.into(),
                        context: None,
                    };
                    let mut body = serde_json::to_vec(&request).unwrap();
                    body.push(b'\n');
                    pipe.write_all(&body).await.unwrap();
                    let response: AuthBrokerResponse =
                        serde_json::from_slice(&read_frame(&mut pipe).await.unwrap()).unwrap();
                    assert!(!response.ok);
                    assert!(response.token.is_none());
                });
            }
            "untrusted" => {
                assert!(request_desktop_token("GET", "/api/mobile/pairing").is_err());
                fs::write(directory.join("untrusted.pid"), std::process::id().to_string()).unwrap();
            }
            _ => panic!("unknown broker child role"),
        }
    }

    #[test]
    fn distinct_processes_enforce_mobile_pairing_route_and_peer_identity() {
        use std::process::{Command, Stdio};

        let current = std::env::current_exe().unwrap();
        let directory = tempfile::Builder::new()
            .prefix("iterate-broker-process-test-")
            .tempdir_in(current.parent().unwrap())
            .unwrap();
        let server_exe = directory.path().join("mobile-bridge-process-test.exe");
        let client_exe = directory.path().join("iterate.exe");
        let untrusted_exe = directory.path().join("untrusted.exe");
        for path in [&server_exe, &client_exe, &untrusted_exe] {
            fs::copy(&current, path).unwrap();
        }
        let test_id = uuid::Uuid::new_v4().simple().to_string();
        let child = |exe: &Path, role: &str| {
            let mut command = Command::new(exe);
            command
                .args([
                    "--exact",
                    "bridge::auth::windows_broker::tests::broker_child_process",
                    "--nocapture",
                ])
                .env("ITERATE_BROKER_TEST_ROLE", role)
                .env("ITERATE_BROKER_TEST_DIR", directory.path())
                .env("ITERATE_BROKER_TEST_PIPE_ID", &test_id)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command
        };
        let mut server = child(&server_exe, "server").spawn().unwrap();
        let ready = directory.path().join("ready");
        for _ in 0..200 {
            if ready.exists() {
                break;
            }
            if server.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(StdDuration::from_millis(50));
        }
        assert!(ready.exists(), "separate broker process failed to start");
        let server_pid: u32 = fs::read_to_string(&ready).unwrap().parse().unwrap();
        for (exe, role) in [
            (&client_exe, "valid"),
            (&client_exe, "denied_method"),
            (&client_exe, "denied_path"),
            (&untrusted_exe, "untrusted"),
        ] {
            let output = child(exe, role).output().unwrap();
            assert!(
                output.status.success(),
                "{role} failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            println!("broker_child_role={role} exit=0");
        }
        let valid: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.path().join("valid.json")).unwrap())
                .unwrap();
        let client_pid = valid["pid"].as_u64().unwrap() as u32;
        let untrusted_pid: u32 = fs::read_to_string(directory.path().join("untrusted.pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(server_pid, client_pid);
        assert_ne!(server_pid, untrusted_pid);
        println!(
            "broker_server_pid={server_pid} trusted_client_pid={client_pid} untrusted_client_pid={untrusted_pid}"
        );
        fs::write(directory.path().join("done"), b"").unwrap();
        let output = server.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "server failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
