use base64::{engine::general_purpose::STANDARD, Engine};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const PORT: u16 = 9000;
const SSDP_ADDR: &str = "239.255.255.250";
const SSDP_PORT: u16 = 1900;
const USB_ROOT: &str = "/tmp/usb";
const SERVER_NAME: &str = "USB Media Server";
const UDN: &str = "uuid:e1a2b3c4-d5e6-f7a8-b9c0-d1e2f3a4b5c6";
const BUF_SIZE: usize = 65536;

static VIDEO_EXT: &[&str] = &["mp4","mkv","avi","mov","webm","ts","flv","wmv","m4v","3gp"];
static AUDIO_EXT: &[&str] = &["mp3","flac","aac","ogg","wav","wma","m4a"];
static IMAGE_EXT: &[&str] = &["jpg","jpeg","png","gif","bmp","webp","tiff"];
static SUB_EXT: &[&str] = &["srt","sub","ass","ssa","vtt"];

fn get_mime(ext: &str) -> &'static str {
    match ext {
        "mp4" => "video/mp4", "mkv" => "video/x-matroska", "avi" => "video/x-msvideo",
        "mov" => "video/quicktime", "webm" => "video/webm", "ts" => "video/mp2t",
        "flv" => "video/x-flv", "wmv" => "video/x-ms-wmv", "m4v" => "video/x-m4v",
        "3gp" => "video/3gpp",
        "mp3" => "audio/mpeg", "flac" => "audio/flac", "aac" => "audio/aac",
        "ogg" => "audio/ogg", "wav" => "audio/wav", "wma" => "audio/x-ms-wma",
        "m4a" => "audio/mp4",
        "jpg" | "jpeg" => "image/jpeg", "png" => "image/png", "gif" => "image/gif",
        "bmp" => "image/bmp", "webp" => "image/webp", "tiff" => "image/tiff",
        "srt" => "text/srt", "sub" => "text/sub", "ass" | "ssa" => "text/x-ass",
        "vtt" => "text/vtt",
        _ => "application/octet-stream",
    }
}

fn get_upnp_class(ext: &str) -> Option<&'static str> {
    if VIDEO_EXT.contains(&ext) { Some("object.item.videoItem") }
    else if AUDIO_EXT.contains(&ext) { Some("object.item.audioItem.musicTrack") }
    else if IMAGE_EXT.contains(&ext) { Some("object.item.imageItem.photo") }
    else { None }
}

fn get_ext(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn dlna_features(mime: &str) -> String {
    let flag = if mime.starts_with("image/") {
        "00D00000000000000000000000000000"
    } else {
        "01700000000000000000000000000000"
    };
    format!("DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS={flag}")
}

fn esc_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn path_to_id(full: &Path, root: &Path) -> String {
    let rel = full.strip_prefix(root).unwrap_or(full);
    STANDARD.encode(format!("/{}", rel.to_string_lossy()))
}

fn id_to_path(id: &str, root: &Path) -> Option<PathBuf> {
    let decoded = STANDARD.decode(id).ok()?;
    let rel = String::from_utf8(decoded).ok()?;
    let rel = rel.trim_start_matches('/');
    if rel.contains("..") { return None; }
    let p = root.join(rel);
    if p.starts_with(root) { Some(p) } else { None }
}

fn find_usb_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let Ok(drives) = fs::read_dir(USB_ROOT) else { return roots };
    for drive in drives.flatten() {
        let dp = drive.path();
        let Ok(parts) = fs::read_dir(&dp) else { continue };
        for part in parts.flatten() {
            let pp = part.path();
            if pp.is_dir() {
                if let Ok(contents) = fs::read_dir(&pp) {
                    if contents.count() > 0 { roots.push(pp); }
                }
            }
        }
    }
    roots
}

fn resolve_id_to_path(id: &str) -> Option<PathBuf> {
    for root in find_usb_roots() {
        if let Some(p) = id_to_path(id, &root) {
            if p.exists() { return Some(p); }
        }
    }
    None
}

fn find_subtitles(video_path: &Path) -> Vec<(PathBuf, String)> {
    let stem = video_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let dir = video_path.parent().unwrap_or(Path::new("."));
    let Ok(entries) = fs::read_dir(dir) else { return vec![] };
    let mut subs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let ext = get_ext(&name);
        if SUB_EXT.contains(&ext.as_str()) && name.starts_with(&stem) {
            subs.push((entry.path(), ext));
        }
    }
    subs
}

fn get_host_ip() -> String {
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.connect("8.8.8.8:80").ok();
    socket.local_addr().map(|a| a.ip().to_string()).unwrap_or_else(|_| "127.0.0.1".to_string())
}

struct Server {
    host_ip: String,
    base_url: String,
}

impl Server {
    fn new() -> Self {
        let host_ip = get_host_ip();
        let base_url = format!("http://{}:{}", host_ip, PORT);
        Self { host_ip, base_url }
    }

    fn browse_root(&self) -> (Vec<DIDLItem>, usize) {
        let roots = find_usb_roots();
        let items: Vec<DIDLItem> = roots.iter().map(|r| {
            let label = r.file_name().unwrap_or_default().to_string_lossy().to_string();
            DIDLItem::Container {
                id: format!("root:{label}"),
                parent_id: "0".to_string(),
                title: label,
            }
        }).collect();
        let total = items.len();
        (items, total)
    }

    fn resolve_effective_root(&self, object_id: &str) -> (PathBuf, String) {
        if object_id.starts_with("root:") {
            let part_name = &object_id[5..];
            for root in find_usb_roots() {
                if root.file_name().map(|f| f.to_string_lossy().to_string()).as_deref() == Some(part_name) {
                    return (root, "0".to_string());
                }
            }
        }
        for root in find_usb_roots() {
            if let Some(p) = id_to_path(object_id, &root) {
                if p.exists() {
                    return (root, object_id.to_string());
                }
            }
        }
        (find_usb_roots().into_iter().next().unwrap_or_default(), object_id.to_string())
    }

    fn browse_children(&self, object_id: &str, start: usize, count: usize) -> (Vec<DIDLItem>, usize) {
        if object_id == "0" {
            return self.browse_root();
        }

        let (root, eff_id) = self.resolve_effective_root(object_id);
        let dir_path = if eff_id == "0" { root.clone() } else {
            match id_to_path(&eff_id, &root) {
                Some(p) => p,
                None => return (vec![], 0),
            }
        };

        let Ok(entries) = fs::read_dir(&dir_path) else { return (vec![], 0) };
        let mut items = Vec::new();

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') { continue; }
            let full = entry.path();
            let id = path_to_id(&full, &root);

            if full.is_dir() {
                items.push(DIDLItem::Container {
                    id,
                    parent_id: object_id.to_string(),
                    title: name,
                });
            } else {
                let ext = get_ext(&name);
                let Some(upnp_class) = get_upnp_class(&ext) else { continue };
                let mime = get_mime(&ext);
                let size = fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
                let url = format!("{}/media/{}", self.base_url, urlenc(&id));

                let subs = if VIDEO_EXT.contains(&ext.as_str()) {
                    find_subtitles(&full).into_iter().map(|(sp, se)| {
                        let sid = path_to_id(&sp, &root);
                        SubInfo {
                            url: format!("{}/sub/{}", self.base_url, urlenc(&sid)),
                            mime: get_mime(&se).to_string(),
                        }
                    }).collect()
                } else { vec![] };

                items.push(DIDLItem::Item {
                    id,
                    parent_id: object_id.to_string(),
                    title: name,
                    upnp_class: upnp_class.to_string(),
                    mime: mime.to_string(),
                    size,
                    url,
                    subs,
                });
            }
        }

        let total = items.len();
        let items = if count > 0 {
            items.into_iter().skip(start).take(count).collect()
        } else {
            items.into_iter().skip(start).collect()
        };
        (items, total)
    }

    fn browse_metadata(&self, object_id: &str) -> String {
        if object_id == "0" {
            return self.didl_container(object_id, "-1", SERVER_NAME);
        }
        if object_id.starts_with("root:") {
            return self.didl_container(object_id, "0", &object_id[5..]);
        }
        if let Some(path) = resolve_id_to_path(object_id) {
            if path.is_dir() {
                let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                return self.didl_container(object_id, "-1", &name);
            }
            let ext = get_ext(&path.to_string_lossy());
            if let Some(upnp_class) = get_upnp_class(&ext) {
                let mime = get_mime(&ext);
                let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let features = dlna_features(mime);
                let url = format!("{}/media/{}", self.base_url, urlenc(object_id));
                let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                return format!(
                    r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/" xmlns:sec="http://www.sec.co.kr/"><item id="{}" parentID="-1" restricted="1"><dc:title>{}</dc:title><upnp:class>{}</upnp:class><res protocolInfo="http-get:*:{}:{}" size="{}">{}</res></item></DIDL-Lite>"#,
                    esc_xml(object_id), esc_xml(&name), upnp_class, mime, features, size, esc_xml(&url)
                );
            }
        }
        self.didl_container(object_id, "-1", object_id)
    }

    fn didl_container(&self, id: &str, parent: &str, title: &str) -> String {
        format!(
            r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/"><container id="{}" parentID="{}" restricted="1"><dc:title>{}</dc:title><upnp:class>object.container.storageFolder</upnp:class></container></DIDL-Lite>"#,
            esc_xml(id), esc_xml(parent), esc_xml(title)
        )
    }

    fn didl_result(&self, items: &[DIDLItem]) -> String {
        let mut s = String::from(r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/" xmlns:sec="http://www.sec.co.kr/">"#);
        for item in items {
            match item {
                DIDLItem::Container { id, parent_id, title } => {
                    s += &format!(
                        r#"<container id="{}" parentID="{}" restricted="1"><dc:title>{}</dc:title><upnp:class>object.container.storageFolder</upnp:class></container>"#,
                        esc_xml(id), esc_xml(parent_id), esc_xml(title)
                    );
                }
                DIDLItem::Item { id, parent_id, title, upnp_class, mime, size, url, subs } => {
                    let features = dlna_features(mime);
                    s += &format!(
                        r#"<item id="{}" parentID="{}" restricted="1"><dc:title>{}</dc:title><upnp:class>{}</upnp:class><res protocolInfo="http-get:*:{}:{}" size="{}">{}</res>"#,
                        esc_xml(id), esc_xml(parent_id), esc_xml(title), upnp_class, mime, features, size, esc_xml(url)
                    );
                    for sub in subs {
                        s += &format!(
                            r#"<sec:CaptionInfoEx sec:type="{}">{}</sec:CaptionInfoEx>"#,
                            esc_xml(&sub.mime), esc_xml(&sub.url)
                        );
                    }
                    s += "</item>";
                }
            }
        }
        s += "</DIDL-Lite>";
        s
    }
}

#[derive(Clone)]
enum DIDLItem {
    Container { id: String, parent_id: String, title: String },
    Item { id: String, parent_id: String, title: String, upnp_class: String, mime: String, size: u64, url: String, subs: Vec<SubInfo> },
}

#[derive(Clone)]
struct SubInfo { url: String, mime: String }

fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out += &format!("%{:02X}", b),
        }
    }
    out
}

fn urldec(s: &str) -> String {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i+1..i+3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn parse_soap_args(body: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let tags = ["ObjectID", "BrowseFlag", "StartingIndex", "RequestedCount", "Filter", "SortCriteria"];
    for tag in &tags {
        let open = format!("<{}>", tag);
        let close = format!("</{}>", tag);
        if let Some(start) = body.find(&open) {
            let start = start + open.len();
            if let Some(end) = body[start..].find(&close) {
                let val = &body[start..start+end];
                let val = val.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"");
                map.insert(tag.to_string(), val);
            }
        }
    }
    map
}

fn soap_response(action: &str, fields: &[(&str, &str)]) -> String {
    let mut body = String::new();
    for (k, v) in fields {
        body += &format!("<{}>{}</{}>", k, esc_xml(v), k);
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{}Response xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1">{}</u:{}Response></s:Body></s:Envelope>"#,
        action, body, action
    )
}

fn cms_soap_response(action: &str, fields: &[(&str, &str)]) -> String {
    let mut body = String::new();
    for (k, v) in fields {
        body += &format!("<{}>{}</{}>", k, esc_xml(v), k);
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{}Response xmlns:u="urn:schemas-upnp-org:service:ConnectionManager:1">{}</u:{}Response></s:Body></s:Envelope>"#,
        action, body, action
    )
}

fn handle_request(stream: &mut TcpStream, server: &Server) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 { return; }

    let parts: Vec<&str> = request_line.trim().splitn(3, ' ').collect();
    if parts.len() < 2 { return; }
    let method = parts[0];
    let raw_path = parts[1];
    let path = raw_path.split('?').next().unwrap_or(raw_path);
    if path.starts_with("/media") || path.starts_with("/cds") {
        eprintln!("{} {} {:?}", method, path, stream.peer_addr().ok());
    }

    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 { break; }
        let line = line.trim().to_string();
        if line.is_empty() { break; }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }

    let content_length: usize = headers.get("content-length")
        .and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; content_length];
    if content_length > 0 { let _ = reader.read_exact(&mut body); }
    let body_str = String::from_utf8_lossy(&body).to_string();

    // Device description
    if path == "/device.xml" {
        let xml = device_xml(server);
        send_response(stream, 200, "text/xml; charset=\"utf-8\"", xml.as_bytes(), &[]);
        return;
    }
    if path == "/cds.xml" {
        send_response(stream, 200, "text/xml; charset=\"utf-8\"", CDS_XML.as_bytes(), &[]);
        return;
    }
    if path == "/cms.xml" {
        send_response(stream, 200, "text/xml; charset=\"utf-8\"", CMS_XML.as_bytes(), &[]);
        return;
    }

    // Event subscriptions
    if (path == "/cds/event" || path == "/cms/event") && method == "SUBSCRIBE" {
        let resp = format!("HTTP/1.1 200 OK\r\nSID: uuid:sub-{}\r\nTIMEOUT: Second-1800\r\n\r\n",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());
        let _ = stream.write_all(resp.as_bytes());
        return;
    }

    // CDS control
    if path == "/cds/control" && method == "POST" {
        let soap_action = headers.get("soapaction").cloned().unwrap_or_default();

        if soap_action.contains("Browse") {
            let args = parse_soap_args(&body_str);
            let object_id = args.get("ObjectID").cloned().unwrap_or_else(|| "0".to_string());
            let browse_flag = args.get("BrowseFlag").cloned().unwrap_or_default();
            let start: usize = args.get("StartingIndex").and_then(|v| v.parse().ok()).unwrap_or(0);
            let count: usize = args.get("RequestedCount").and_then(|v| v.parse().ok()).unwrap_or(0);

            let resp_xml = if browse_flag == "BrowseMetadata" {
                let didl = server.browse_metadata(&object_id);
                soap_response("Browse", &[
                    ("Result", &didl), ("NumberReturned", "1"), ("TotalMatches", "1"), ("UpdateID", "1"),
                ])
            } else {
                let (items, total) = server.browse_children(&object_id, start, count);
                let didl = server.didl_result(&items);
                soap_response("Browse", &[
                    ("Result", &didl),
                    ("NumberReturned", &items.len().to_string()),
                    ("TotalMatches", &total.to_string()),
                    ("UpdateID", "1"),
                ])
            };
            send_response(stream, 200, "text/xml; charset=\"utf-8\"", resp_xml.as_bytes(), &[]);
        } else if soap_action.contains("GetSearchCapabilities") {
            let r = soap_response("GetSearchCapabilities", &[("SearchCaps", "")]);
            send_response(stream, 200, "text/xml; charset=\"utf-8\"", r.as_bytes(), &[]);
        } else if soap_action.contains("GetSystemUpdateID") {
            let r = soap_response("GetSystemUpdateID", &[("Id", "1")]);
            send_response(stream, 200, "text/xml; charset=\"utf-8\"", r.as_bytes(), &[]);
        } else if soap_action.contains("GetSortCapabilities") {
            let r = soap_response("GetSortCapabilities", &[("SortCaps", "")]);
            send_response(stream, 200, "text/xml; charset=\"utf-8\"", r.as_bytes(), &[]);
        } else {
            send_response(stream, 200, "text/xml; charset=\"utf-8\"",
                b"<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body></s:Body></s:Envelope>", &[]);
        }
        return;
    }

    // CMS control
    if path == "/cms/control" && method == "POST" {
        let soap_action = headers.get("soapaction").cloned().unwrap_or_default();
        let df = "DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000";
        if soap_action.contains("GetProtocolInfo") {
            let protocols = format!(
                "http-get:*:video/mp4:{df},http-get:*:video/x-matroska:{df},http-get:*:video/x-msvideo:{df},http-get:*:video/quicktime:{df},http-get:*:audio/mpeg:{df},http-get:*:audio/flac:{df},http-get:*:image/jpeg:{df},http-get:*:image/png:{df}"
            );
            let r = cms_soap_response("GetProtocolInfo", &[("Source", &protocols), ("Sink", "")]);
            send_response(stream, 200, "text/xml; charset=\"utf-8\"", r.as_bytes(), &[]);
        } else {
            send_response(stream, 200, "text/xml; charset=\"utf-8\"",
                b"<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body></s:Body></s:Envelope>", &[]);
        }
        return;
    }

    // Subtitle serving
    if let Some(sub_id) = path.strip_prefix("/sub/") {
        let sub_id = urldec(sub_id);
        if let Some(sub_path) = resolve_id_to_path(&sub_id) {
            if let Ok(data) = fs::read(&sub_path) {
                let ext = get_ext(&sub_path.to_string_lossy());
                let mime = get_mime(&ext);
                send_response(stream, 200, mime, &data, &[("Access-Control-Allow-Origin", "*")]);
                return;
            }
        }
        send_response(stream, 404, "text/plain", b"Not found", &[]);
        return;
    }

    // Media streaming
    if let Some(file_id) = path.strip_prefix("/media/") {
        let file_id = urldec(file_id);
        let Some(file_path) = resolve_id_to_path(&file_id) else {
            send_response(stream, 404, "text/plain", b"Not found", &[]);
            return;
        };
        let Ok(meta) = fs::metadata(&file_path) else {
            send_response(stream, 404, "text/plain", b"Not found", &[]);
            return;
        };
        let file_size = meta.len();
        let ext = get_ext(&file_path.to_string_lossy());
        let mime = get_mime(&ext);
        let features = dlna_features(mime);

        // HEAD
        if method == "HEAD" {
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\ntransferMode.dlna.org: Streaming\r\ncontentFeatures.dlna.org: {}\r\nConnection: close\r\n\r\n",
                file_size, mime, features
            );
            let _ = stream.write_all(header.as_bytes());
            return;
        }

        // Range handling
        let range = headers.get("range").cloned();
        eprintln!("  Range: {:?} Size: {}", range, file_size);
        const MAX_CHUNK: u64 = 2_000_000_000;
        if let Some(range_str) = range {
            let range_str = range_str.replace("bytes=", "");
            let parts: Vec<&str> = range_str.splitn(2, '-').collect();
            let start: u64 = parts[0].parse().unwrap_or(0);
            let open_ended = parts.len() <= 1 || parts[1].is_empty();
            let end: u64 = if !open_ended {
                parts[1].parse().unwrap_or(file_size - 1)
            } else if start == 0 {
                file_size - 1
            } else {
                (start + MAX_CHUNK - 1).min(file_size - 1)
            };
            let end = end.min(file_size - 1);
            let chunk = end - start + 1;

            let header = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nContent-Type: {}\r\ntransferMode.dlna.org: Streaming\r\ncontentFeatures.dlna.org: {}\r\nConnection: close\r\n\r\n",
                start, end, file_size, chunk, mime, features
            );
            let _ = stream.write_all(header.as_bytes());
            stream_file(stream, &file_path, start, chunk);
        } else {
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\ntransferMode.dlna.org: Streaming\r\ncontentFeatures.dlna.org: {}\r\nConnection: close\r\n\r\n",
                file_size, mime, features
            );
            let _ = stream.write_all(header.as_bytes());
            stream_file(stream, &file_path, 0, file_size);
        }
        return;
    }

    send_response(stream, 404, "text/plain", b"Not found", &[]);
}

fn stream_file(stream: &mut TcpStream, path: &Path, offset: u64, length: u64) {
    let Ok(mut file) = fs::File::open(path) else { return };
    if offset > 0 { let _ = file.seek(SeekFrom::Start(offset)); }
    let mut buf = [0u8; BUF_SIZE];
    let mut remaining = length;
    while remaining > 0 {
        let to_read = (remaining as usize).min(BUF_SIZE);
        match file.read(&mut buf[..to_read]) {
            Ok(0) => break,
            Ok(n) => {
                if stream.write_all(&buf[..n]).is_err() { break; }
                remaining -= n as u64;
            }
            Err(_) => break,
        }
    }
}

fn send_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8], extra_headers: &[(&str, &str)]) {
    let status_text = match status {
        200 => "OK", 206 => "Partial Content", 404 => "Not Found", 403 => "Forbidden", _ => "OK",
    };
    let mut header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        status, status_text, content_type, body.len()
    );
    for (k, v) in extra_headers {
        header += &format!("{}: {}\r\n", k, v);
    }
    header += "\r\n";
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn device_xml(_server: &Server) -> String {
    format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <device>
    <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
    <friendlyName>{name}</friendlyName>
    <manufacturer>Custom</manufacturer>
    <modelName>DLNA Server</modelName>
    <UDN>{udn}</UDN>
    <serviceList>
      <service>
        <serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId>
        <SCPDURL>/cds.xml</SCPDURL>
        <controlURL>/cds/control</controlURL>
        <eventSubURL>/cds/event</eventSubURL>
      </service>
      <service>
        <serviceType>urn:schemas-upnp-org:service:ConnectionManager:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:ConnectionManager</serviceId>
        <SCPDURL>/cms.xml</SCPDURL>
        <controlURL>/cms/control</controlURL>
        <eventSubURL>/cms/event</eventSubURL>
      </service>
    </serviceList>
  </device>
</root>"#, name = SERVER_NAME, udn = UDN)
}

const CDS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <actionList>
    <action><name>Browse</name>
      <argumentList>
        <argument><name>ObjectID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ObjectID</relatedStateVariable></argument>
        <argument><name>BrowseFlag</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_BrowseFlag</relatedStateVariable></argument>
        <argument><name>Filter</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Filter</relatedStateVariable></argument>
        <argument><name>StartingIndex</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Index</relatedStateVariable></argument>
        <argument><name>RequestedCount</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
        <argument><name>SortCriteria</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_SortCriteria</relatedStateVariable></argument>
        <argument><name>Result</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Result</relatedStateVariable></argument>
        <argument><name>NumberReturned</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
        <argument><name>TotalMatches</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
        <argument><name>UpdateID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_UpdateID</relatedStateVariable></argument>
      </argumentList>
    </action>
    <action><name>GetSearchCapabilities</name><argumentList><argument><name>SearchCaps</name><direction>out</direction><relatedStateVariable>SearchCapabilities</relatedStateVariable></argument></argumentList></action>
    <action><name>GetSortCapabilities</name><argumentList><argument><name>SortCaps</name><direction>out</direction><relatedStateVariable>SortCapabilities</relatedStateVariable></argument></argumentList></action>
    <action><name>GetSystemUpdateID</name><argumentList><argument><name>Id</name><direction>out</direction><relatedStateVariable>SystemUpdateID</relatedStateVariable></argument></argumentList></action>
  </actionList>
  <serviceStateTable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ObjectID</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Result</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_BrowseFlag</name><dataType>string</dataType><allowedValueList><allowedValue>BrowseMetadata</allowedValue><allowedValue>BrowseDirectChildren</allowedValue></allowedValueList></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Filter</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_SortCriteria</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Index</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Count</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_UpdateID</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="yes"><name>SystemUpdateID</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>SearchCapabilities</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>SortCapabilities</name><dataType>string</dataType></stateVariable>
  </serviceStateTable>
</scpd>"#;

const CMS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <actionList>
    <action><name>GetProtocolInfo</name>
      <argumentList>
        <argument><name>Source</name><direction>out</direction><relatedStateVariable>SourceProtocolInfo</relatedStateVariable></argument>
        <argument><name>Sink</name><direction>out</direction><relatedStateVariable>SinkProtocolInfo</relatedStateVariable></argument>
      </argumentList>
    </action>
    <action><name>GetCurrentConnectionIDs</name><argumentList><argument><name>ConnectionIDs</name><direction>out</direction><relatedStateVariable>CurrentConnectionIDs</relatedStateVariable></argument></argumentList></action>
    <action><name>GetCurrentConnectionInfo</name>
      <argumentList>
        <argument><name>ConnectionID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ConnectionID</relatedStateVariable></argument>
        <argument><name>RcsID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_RcsID</relatedStateVariable></argument>
        <argument><name>AVTransportID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_AVTransportID</relatedStateVariable></argument>
        <argument><name>ProtocolInfo</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ProtocolInfo</relatedStateVariable></argument>
        <argument><name>PeerConnectionManager</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionManager</relatedStateVariable></argument>
        <argument><name>PeerConnectionID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionID</relatedStateVariable></argument>
        <argument><name>Direction</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Direction</relatedStateVariable></argument>
        <argument><name>Status</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionStatus</relatedStateVariable></argument>
      </argumentList>
    </action>
  </actionList>
  <serviceStateTable>
    <stateVariable sendEvents="yes"><name>SourceProtocolInfo</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="yes"><name>SinkProtocolInfo</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="yes"><name>CurrentConnectionIDs</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionStatus</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionManager</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Direction</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ProtocolInfo</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionID</name><dataType>i4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_AVTransportID</name><dataType>i4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_RcsID</name><dataType>i4</dataType></stateVariable>
  </serviceStateTable>
</scpd>"#;

fn ssdp_advertise(server: &Server) {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else { return };
    let addr: SocketAddr = format!("{}:{}", SSDP_ADDR, SSDP_PORT).parse().unwrap();
    let location = format!("{}/device.xml", server.base_url);

    let messages = [
        format!("NOTIFY * HTTP/1.1\r\nHOST: {SSDP_ADDR}:{SSDP_PORT}\r\nNT: upnp:rootdevice\r\nNTS: ssdp:alive\r\nUSN: {UDN}::upnp:rootdevice\r\nLOCATION: {location}\r\nSERVER: Linux/4.4 UPnP/1.0 DLNAServer/1.0\r\nCACHE-CONTROL: max-age=1800\r\n\r\n"),
        format!("NOTIFY * HTTP/1.1\r\nHOST: {SSDP_ADDR}:{SSDP_PORT}\r\nNT: urn:schemas-upnp-org:device:MediaServer:1\r\nNTS: ssdp:alive\r\nUSN: {UDN}::urn:schemas-upnp-org:device:MediaServer:1\r\nLOCATION: {location}\r\nSERVER: Linux/4.4 UPnP/1.0 DLNAServer/1.0\r\nCACHE-CONTROL: max-age=1800\r\n\r\n"),
        format!("NOTIFY * HTTP/1.1\r\nHOST: {SSDP_ADDR}:{SSDP_PORT}\r\nNT: urn:schemas-upnp-org:service:ContentDirectory:1\r\nNTS: ssdp:alive\r\nUSN: {UDN}::urn:schemas-upnp-org:service:ContentDirectory:1\r\nLOCATION: {location}\r\nSERVER: Linux/4.4 UPnP/1.0 DLNAServer/1.0\r\nCACHE-CONTROL: max-age=1800\r\n\r\n"),
    ];

    loop {
        for msg in &messages {
            let _ = socket.send_to(msg.as_bytes(), addr);
        }
        thread::sleep(Duration::from_secs(30));
    }
}

fn ssdp_respond(server: Arc<Server>) {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
    sock.set_reuse_address(true).unwrap();
    let addr: SocketAddr = format!("0.0.0.0:{SSDP_PORT}").parse().unwrap();
    sock.bind(&addr.into()).unwrap();
    let socket: UdpSocket = sock.into();
    let mcast = SSDP_ADDR.parse::<Ipv4Addr>().unwrap();
    let _ = socket.join_multicast_v4(&mcast, &Ipv4Addr::UNSPECIFIED);
    eprintln!("SSDP listening on {}:{}", SSDP_ADDR, SSDP_PORT);
    let location = format!("{}/device.xml", server.base_url);

    let mut buf = [0u8; 2048];
    loop {
        let Ok((n, src)) = socket.recv_from(&mut buf) else { continue };
        let msg = String::from_utf8_lossy(&buf[..n]);
        if !msg.contains("M-SEARCH") { continue; }
        if !msg.contains("ssdp:discover") { continue; }
        if !msg.contains("upnp:rootdevice") && !msg.contains("ssdp:all")
            && !msg.contains("MediaServer") && !msg.contains("ContentDirectory") { continue; }

        let resp = format!(
            "HTTP/1.1 200 OK\r\nLOCATION: {location}\r\nST: upnp:rootdevice\r\nUSN: {UDN}::upnp:rootdevice\r\nSERVER: Linux/4.4 UPnP/1.0 DLNAServer/1.0\r\nCACHE-CONTROL: max-age=1800\r\n\r\n"
        );
        let _ = socket.send_to(resp.as_bytes(), src);
    }
}

fn main() {
    let server = Arc::new(Server::new());
    eprintln!("DLNA media server running on {}", server.base_url);
    eprintln!("Host IP: {}", server.host_ip);

    let s1 = Arc::clone(&server);
    thread::spawn(move || ssdp_advertise(&s1));

    let s2 = Arc::clone(&server);
    thread::spawn(move || ssdp_respond(s2));

    let tcp_sock = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
    tcp_sock.set_reuse_address(true).unwrap();
    let tcp_addr: SocketAddr = format!("0.0.0.0:{PORT}").parse().unwrap();
    tcp_sock.bind(&tcp_addr.into()).unwrap();
    tcp_sock.listen(128).unwrap();
    let listener: TcpListener = tcp_sock.into();
    eprintln!("Listening on port {PORT}");

    for stream in listener.incoming().flatten() {
        let s = Arc::clone(&server);
        thread::spawn(move || {
            let mut stream = stream;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            handle_request(&mut stream, &s);
        });
    }
}
