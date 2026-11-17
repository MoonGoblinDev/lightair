//! A sign-in brought over from the person's own browser (Chrome, Brave, Edge, Arc on macOS),
//! for a platform whose sign-in won't work in Lightair's window (passkeys with Touch ID): only
//! that platform's cookies, read from a copy of the browser's cookie store and decrypted with
//! its "Safe Storage" key (macOS asks once whether Lightair may), then given to Lightair's
//! browser. Nothing else is read or kept.

/// (name, folder under Application Support, Keychain service, account)
#[cfg(target_os = "macos")]
const BROWSERS: &[(&str, &str, &str, &str)] = &[
    ("Chrome", "Google/Chrome", "Chrome Safe Storage", "Chrome"),
    ("Brave", "BraveSoftware/Brave-Browser", "Brave Safe Storage", "Brave"),
    ("Edge", "Microsoft Edge", "Microsoft Edge Safe Storage", "Microsoft Edge"),
    ("Arc", "Arc/User Data", "Arc Safe Storage", "Arc"),
];

/// The sites a platform's sign-in lives on.
pub fn domains(platform: &str) -> &'static [&'static str] {
    match platform {
        "twitch" => &["twitch.tv"],
        "youtube" | "youtube-shorts" => &["google.com", "youtube.com"],
        "kick" => &["kick.com"],
        "tiktok" => &["tiktok.com"],
        _ => &[],
    }
}

#[cfg(target_os = "macos")]
fn store(folder: &str) -> Option<std::path::PathBuf> {
    let base = std::env::var_os("HOME").map(std::path::PathBuf::from)?.join("Library/Application Support").join(folder).join("Default");
    [base.join("Cookies"), base.join("Network/Cookies")].into_iter().find(|p| p.exists())
}

/// The browsers here that have a cookie store.
pub fn installed() -> Vec<&'static str> {
    #[cfg(target_os = "macos")]
    {
        BROWSERS.iter().filter(|b| store(b.1).is_some()).map(|b| b.0).collect()
    }
    #[cfg(windows)]
    {
        win::BROWSERS.iter().filter(|b| win::store(b.1).is_some()).map(|b| b.0).collect()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    Vec::new()
}

/// Windows: the browser's key is in its `Local State`, sealed with DPAPI (only this Windows
/// user can open it); cookies are AES-256-GCM under it ("v10"/"v11"). Cookies sealed with
/// Chrome's app-bound encryption ("v20", Chrome 127 and later, by design readable only by the
/// browser itself) can't be brought over: those sign-ins are made in Lightair's own window.
#[cfg(windows)]
mod win {
    use std::path::PathBuf;

    /// (name, folder under %LOCALAPPDATA% holding "User Data")
    pub const BROWSERS: &[(&str, &str)] = &[
        ("Chrome", r"Google\Chrome"),
        ("Edge", r"Microsoft\Edge"),
        ("Brave", r"BraveSoftware\Brave-Browser"),
    ];

    fn user_data(folder: &str) -> Option<PathBuf> {
        Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(folder).join("User Data"))
    }

    /// The browser's cookie store (its default profile).
    pub fn store(folder: &str) -> Option<PathBuf> {
        let base = user_data(folder)?.join("Default");
        [base.join(r"Network\Cookies"), base.join("Cookies")].into_iter().find(|p| p.exists())
    }

    /// DPAPI's unseal, for this Windows user.
    fn unprotect(sealed: &[u8]) -> Option<Vec<u8>> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};
        let input = CRYPT_INTEGER_BLOB { cbData: sealed.len() as u32, pbData: sealed.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut out)
        };
        if ok == 0 || out.pbData.is_null() {
            return None;
        }
        let plain = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as _) };
        Some(plain)
    }

    /// The browser's cookie key, from its Local State.
    pub fn key(folder: &str) -> Result<[u8; 32], String> {
        use base64::Engine;
        let path = user_data(folder).ok_or("no LOCALAPPDATA")?.join("Local State");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("couldn't read the browser's Local State: {e}"))?;
        let state: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let sealed = state["os_crypt"]["encrypted_key"].as_str().ok_or("the browser keeps no cookie key")?;
        let sealed = base64::engine::general_purpose::STANDARD.decode(sealed).map_err(|e| e.to_string())?;
        let sealed = sealed.strip_prefix(b"DPAPI").ok_or("the browser's cookie key is sealed some other way")?;
        let key = unprotect(sealed).ok_or("Windows didn't unseal the browser's cookie key")?;
        key.try_into().map_err(|_| "the browser's cookie key isn't 32 bytes".into())
    }

    /// A cookie's value: "v10"/"v11" are AES-256-GCM (12-byte nonce, then the sealed value);
    /// newer browsers put the host's SHA-256 first. `Err(true)`: app-bound ("v20").
    pub fn decrypt(key: &[u8; 32], host: &str, data: &[u8]) -> Result<String, bool> {
        use aes_gcm::aead::{Aead, KeyInit};
        use sha2::Digest;
        if data.starts_with(b"v20") {
            return Err(true);
        }
        let body = data.strip_prefix(b"v10").or_else(|| data.strip_prefix(b"v11")).ok_or(false)?;
        if body.len() < 12 + 16 {
            return Err(false);
        }
        let (nonce, sealed) = body.split_at(12);
        let cipher = aes_gcm::Aes256Gcm::new(key.into());
        let plain = cipher.decrypt(nonce.into(), sealed).map_err(|_| false)?;
        let hash = sha2::Sha256::digest(host.as_bytes());
        let plain = plain.strip_prefix(hash.as_slice()).unwrap_or(&plain);
        String::from_utf8(plain.to_vec()).map_err(|_| false)
    }

    /// A cookie row: host, name, encrypted value, path, expiry, secure, http-only, same-site, value.
    type Row = (String, String, Vec<u8>, String, i64, bool, bool, i64, String);

    /// `platform`'s cookies from `browser`.
    pub fn read(browser: &str, platform: &str) -> Result<Vec<lightair_web::CookieSpec>, String> {
        let (name, folder) = *BROWSERS.iter().find(|b| b.0 == browser).ok_or("unknown browser")?;
        let db = store(folder).ok_or_else(|| format!("{name} isn't installed"))?;
        let wanted = super::domains(platform);
        if wanted.is_empty() {
            return Err(format!("signing in to {platform} isn't supported"));
        }
        let key = key(folder)?;
        // a copy: the browser keeps its own open (while it runs, Windows may refuse even that)
        let copy = std::env::temp_dir().join(format!("lightair-cookies-{}.db", std::process::id()));
        std::fs::copy(&db, &copy).map_err(|_| format!("{name} is using its cookies: close {name} and try again"))?;
        #[cfg(lightair_cross_check)]
        let rows: Result<Vec<Row>, String> = Err("this build has no SQLite".into());
        #[cfg(not(lightair_cross_check))]
        let rows = (|| -> rusqlite::Result<Vec<Row>> {
            let conn = rusqlite::Connection::open_with_flags(&copy, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let filter = wanted.iter().map(|d| format!("host_key = '{d}' OR host_key LIKE '%.{d}'")).collect::<Vec<_>>().join(" OR ");
            let sql = format!(
                "SELECT host_key, name, encrypted_value, path, expires_utc, is_secure, is_httponly, samesite, value FROM cookies WHERE {filter}"
            );
            let mut q = conn.prepare(&sql)?;
            let rows = q
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })();
        let _ = std::fs::remove_file(&copy);
        let rows = rows.map_err(|e| format!("couldn't read {name}'s cookies: {e}"))?;
        let mut list = Vec::new();
        let mut app_bound = 0;
        for (host, cname, sealed, path, expires, secure, http_only, same_site, plain) in rows {
            let value = if sealed.is_empty() {
                plain
            } else {
                match decrypt(&key, &host, &sealed) {
                    Ok(v) => v,
                    Err(true) => {
                        app_bound += 1;
                        continue;
                    }
                    Err(false) => continue,
                }
            };
            list.push(lightair_web::CookieSpec {
                url: format!("https://{}{}", host.trim_start_matches('.'), if path.is_empty() { "/" } else { &path }),
                name: cname,
                value,
                domain: if host.starts_with('.') { host.clone() } else { String::new() },
                path,
                secure,
                http_only,
                expires,
                same_site: same_site as i32 + 1,
            });
        }
        if list.is_empty() && app_bound > 0 {
            return Err(format!(
                "{name} keeps its sign-ins locked to itself (app-bound encryption): sign in to {platform} in Lightair's window instead"
            ));
        }
        if list.is_empty() {
            return Err(format!("{name} isn't signed in to {platform}"));
        }
        Ok(list)
    }
}

#[cfg(target_os = "macos")]
fn decrypt(key: &[u8; 16], host: &str, data: &[u8]) -> Option<String> {
    use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
    use sha2::Digest;
    let body = data.strip_prefix(b"v10")?;
    let mut buf = body.to_vec();
    let plain = cbc::Decryptor::<aes::Aes128>::new(key.into(), &[b' '; 16].into()).decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?;
    // newer Chrome puts the host's SHA-256 first
    let hash = sha2::Sha256::digest(host.as_bytes());
    let plain = plain.strip_prefix(hash.as_slice()).unwrap_or(plain);
    String::from_utf8(plain.to_vec()).ok()
}

/// Reads `platform`'s cookies from `browser` (asks macOS for its key: may wait on the person):
/// the cookies, ready for Lightair's browser.
#[cfg(target_os = "macos")]
pub fn read(browser: &str, platform: &str) -> Result<Vec<lightair_web::CookieSpec>, String> {
    let (name, folder, service, account) = *BROWSERS.iter().find(|b| b.0 == browser).ok_or("unknown browser")?;
    let db = store(folder).ok_or_else(|| format!("{name} isn't installed"))?;
    let wanted = domains(platform);
    if wanted.is_empty() {
        return Err(format!("signing in to {platform} isn't supported"));
    }
    let out = std::process::Command::new("/usr/bin/security").args(["find-generic-password", "-w", "-s", service, "-a", account]).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("macOS didn't give Lightair {name}'s key (Always Allow or Allow, then try again)"));
    }
    let password = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password.as_bytes(), b"saltysalt", 1003, &mut key);
    // a copy: the browser keeps its own locked
    let copy = std::env::temp_dir().join(format!("lightair-cookies-{}.db", std::process::id()));
    std::fs::copy(&db, &copy).map_err(|e| format!("couldn't read {name}'s cookies: {e}"))?;
    let filter = wanted.iter().map(|d| format!("host_key = '{d}' OR host_key LIKE '%.{d}'")).collect::<Vec<_>>().join(" OR ");
    let sql = format!("SELECT host_key, name, hex(encrypted_value), path, expires_utc, is_secure, is_httponly, samesite, value FROM cookies WHERE {filter}");
    let out = std::process::Command::new("/usr/bin/sqlite3").args(["-readonly", "-separator", "\u{1f}"]).arg(&copy).arg(&sql).output();
    let _ = std::fs::remove_file(&copy);
    let out = out.map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut list = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\u{1f}').collect();
        let [host, cname, hex, path, expires, secure, http_only, same_site, plain] = f.as_slice() else { continue };
        let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect();
        let value = if bytes.is_empty() { plain.to_string() } else { match decrypt(&key, host, &bytes) { Some(v) => v, None => continue } };
        list.push(lightair_web::CookieSpec {
            url: format!("https://{}{}", host.trim_start_matches('.'), if path.is_empty() { "/" } else { path }),
            name: cname.to_string(),
            value,
            // a host-only cookie stays one
            domain: if host.starts_with('.') { host.to_string() } else { String::new() },
            path: path.to_string(),
            secure: *secure == "1",
            http_only: *http_only == "1",
            expires: expires.parse().unwrap_or(0),
            same_site: same_site.parse::<i32>().map_or(0, |s| s + 1),
        });
    }
    if list.is_empty() {
        return Err(format!("{name} isn't signed in to {platform}"));
    }
    Ok(list)
}

#[cfg(windows)]
pub fn read(browser: &str, platform: &str) -> Result<Vec<lightair_web::CookieSpec>, String> {
    win::read(browser, platform)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn read(_: &str, _: &str) -> Result<Vec<lightair_web::CookieSpec>, String> {
    Err("bringing a sign-in over from a browser isn't built for this system".into())
}

/// Reads and gives Lightair's browser `platform`'s sign-in from `browser`: how many cookies.
pub fn import(browser: &str, platform: &str) -> Result<usize, String> {
    let list = read(browser, platform)?;
    let (tx, rx) = std::sync::mpsc::channel();
    lightair_web::set_cookies(list, move |n| {
        let _ = tx.send(n);
    });
    let n = rx.recv_timeout(std::time::Duration::from_secs(10)).map_err(|_| "Lightair's browser didn't answer".to_string())?;
    if n == 0 {
        return Err("Lightair's browser took none of them".into());
    }
    Ok(n)
}

#[cfg(all(test, target_os = "macos"))]
mod checks {
    use super::{decrypt, domains};
    use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
    use sha2::Digest;

    fn seal(key: &[u8; 16], plain: &[u8]) -> Vec<u8> {
        let mut out = b"v10".to_vec();
        let mut buf = vec![0u8; plain.len() + 16];
        buf[..plain.len()].copy_from_slice(plain);
        let n = cbc::Encryptor::<aes::Aes128>::new(key.into(), &[b' '; 16].into()).encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len()).unwrap().len();
        out.extend(&buf[..n]);
        out
    }

    #[std::prelude::v1::test]
    fn reads_old_and_new_chrome_values() {
        let mut key = [0u8; 16];
        pbkdf2::pbkdf2_hmac::<sha1::Sha1>(b"peanuts", b"saltysalt", 1003, &mut key);
        assert_eq!(decrypt(&key, ".twitch.tv", &seal(&key, b"abc123")).as_deref(), Some("abc123"));
        let mut hashed = sha2::Sha256::digest(b".twitch.tv").to_vec();
        hashed.extend(b"abc123");
        assert_eq!(decrypt(&key, ".twitch.tv", &seal(&key, &hashed)).as_deref(), Some("abc123"));
        assert_eq!(decrypt(&[0; 16], ".twitch.tv", &seal(&key, b"abc123")), None);
        assert!(domains("youtube").contains(&"google.com"));
        assert!(domains("bilibili").is_empty());
    }
}

#[cfg(all(test, windows))]
mod windows_checks {
    use super::win::decrypt;
    use aes_gcm::aead::{Aead, KeyInit};
    use sha2::Digest;

    fn seal(key: &[u8; 32], nonce: &[u8; 12], plain: &[u8]) -> Vec<u8> {
        let mut out = b"v10".to_vec();
        out.extend(nonce);
        out.extend(aes_gcm::Aes256Gcm::new(key.into()).encrypt(nonce.into(), plain).unwrap());
        out
    }

    #[test]
    fn reads_gcm_values_and_knows_app_bound_ones() {
        let key = [7u8; 32];
        assert_eq!(decrypt(&key, ".twitch.tv", &seal(&key, &[1; 12], b"abc123")), Ok("abc123".into()));
        let mut hashed = sha2::Sha256::digest(b".twitch.tv").to_vec();
        hashed.extend(b"abc123");
        assert_eq!(decrypt(&key, ".twitch.tv", &seal(&key, &[2; 12], &hashed)), Ok("abc123".into()));
        assert_eq!(decrypt(&[0; 32], ".twitch.tv", &seal(&key, &[1; 12], b"abc123")), Err(false), "another key");
        assert_eq!(decrypt(&key, ".twitch.tv", b"v20\x01\x02\x03"), Err(true), "app-bound");
    }

    #[test]
    #[ignore = "reads this PC's browsers (counts only); cargo xtask test --machine"]
    fn this_pcs_browsers() {
        for b in super::installed() {
            for platform in ["twitch", "youtube"] {
                match super::read(b, platform) {
                    Ok(list) => eprintln!("{b}: {} {platform} cookies read", list.len()),
                    Err(e) => eprintln!("{b}: {e}"),
                }
            }
        }
    }
}
