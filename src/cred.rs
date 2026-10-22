use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use crate::config;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const PROTOCOL_NAME: &str = "qrcred";
const CDP_PORT_BASE: u16 = 9333;

static CDP_PORT_CELL: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

fn is_cdp_alive(port: u16) -> bool {
    http_req(port, "GET", "/json/version")
        .map(|b| b.contains("webSocketDebuggerUrl"))
        .unwrap_or(false)
}

fn port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

fn cdp_port() -> u16 {
    *CDP_PORT_CELL.get_or_init(|| {
        if is_cdp_alive(CDP_PORT_BASE) {
            return CDP_PORT_BASE;
        }
        for p in CDP_PORT_BASE..CDP_PORT_BASE + 20 {
            if port_free(p) {
                return p;
            }
        }
        CDP_PORT_BASE
    })
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Creds {
    pub grabbed_at: u64,
    pub ua: Option<String>,
    pub ucparams: Option<Value>,
    pub account: Option<Value>,
    pub public_params: Option<Value>,
    pub base_info: Option<Value>,
    pub sdk: Option<Value>,
    pub has_ucapi: Option<bool>,
    pub cookies: Option<Vec<Value>>,
}

pub fn cred_path() -> PathBuf {
    config::app_dir().join("creds.json")
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn gen_key() -> String {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let mut a = RandomState::new().build_hasher();
    a.write_u64(std::process::id() as u64);
    let mut b = RandomState::new().build_hasher();
    b.write_u64(now_unix());
    format!("{:016x}{:016x}", a.finish(), b.finish())
}

fn rand_bytes(n: usize) -> Vec<u8> {
    use std::hash::{BuildHasher, Hasher, RandomState};
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let mut h = RandomState::new().build_hasher();
        h.write(&out);
        h.write_u64(now_unix());
        h.write_u64(std::process::id() as u64);
        for b in h.finish().to_le_bytes() {
            if out.len() < n {
                out.push(b);
            }
        }
    }
    out
}

const B64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64_CHARS[((n >> 18) & 63) as usize] as char);
        out.push(B64_CHARS[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64_CHARS[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64_CHARS[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}


struct WsConn {
    stream: TcpStream,
}

impl WsConn {
    fn connect(port: u16, path: &str) -> Result<WsConn, String> {
        let mut stream =
            TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("连接调试端口失败：{e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(90)))
            .map_err(|e| e.to_string())?;
        let key = b64_encode(&rand_bytes(16));
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream
            .write_all(req.as_bytes())
            .map_err(|e| format!("发送握手失败：{e}"))?;
        let mut buf: Vec<u8> = Vec::new();
        let mut b = [0u8; 1];
        loop {
            match stream.read(&mut b) {
                Ok(0) => return Err("调试端口连接中断".into()),
                Ok(_) => {
                    buf.push(b[0]);
                    if buf.ends_with(b"\r\n\r\n") {
                        break;
                    }
                    if buf.len() > 8192 {
                        return Err("握手响应过大".into());
                    }
                }
                Err(e) => return Err(format!("读取握手失败：{e}")),
            }
        }
        let head = String::from_utf8_lossy(&buf);
        if !head.starts_with("HTTP/1.1 101") {
            return Err(format!(
                "WebSocket 握手失败：{}",
                head.lines().next().unwrap_or("")
            ));
        }
        Ok(WsConn { stream })
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), String> {
        let mask = rand_bytes(4);
        let mut frame = vec![0x80 | opcode];
        if payload.len() < 126 {
            frame.push(0x80 | payload.len() as u8);
        } else if payload.len() <= 0xFFFF {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
        frame.extend_from_slice(&mask);
        for (i, b) in payload.iter().enumerate() {
            frame.push(b ^ mask[i % 4]);
        }
        self.stream
            .write_all(&frame)
            .map_err(|e| format!("发送数据失败：{e}"))
    }

    fn send_text(&mut self, text: &str) -> Result<(), String> {
        self.send_frame(0x1, text.as_bytes())
    }

    fn set_read_timeout(&mut self, secs: u64) {
        let _ = self
            .stream
            .set_read_timeout(Some(Duration::from_secs(secs)));
    }

    fn read_exact_n(&mut self, n: usize) -> Result<Vec<u8>, String> {
        let mut out = vec![0u8; n];
        let mut got = 0;
        while got < n {
            match self.stream.read(&mut out[got..]) {
                Ok(0) => return Err("调试连接中断".into()),
                Ok(k) => got += k,
                Err(e) => return Err(format!("读取数据失败：{e}")),
            }
        }
        Ok(out)
    }

    fn recv_text(&mut self) -> Result<String, String> {
        let mut acc: Vec<u8> = Vec::new();
        loop {
            let hdr = self.read_exact_n(2)?;
            let fin = hdr[0] & 0x80 != 0;
            let opcode = hdr[0] & 0x0F;
            let masked = hdr[1] & 0x80 != 0;
            let mut len = (hdr[1] & 0x7F) as u64;
            if len == 126 {
                let e = self.read_exact_n(2)?;
                len = u16::from_be_bytes([e[0], e[1]]) as u64;
            } else if len == 127 {
                let e = self.read_exact_n(8)?;
                len = u64::from_be_bytes(e.try_into().unwrap());
            }
            let mask = if masked { Some(self.read_exact_n(4)?) } else { None };
            let mut data = self.read_exact_n(len as usize)?;
            if let Some(m) = &mask {
                for (i, b) in data.iter_mut().enumerate() {
                    *b ^= m[i % 4];
                }
            }
            match opcode {
                0x8 => return Err("调试 WebSocket 已关闭".into()),
                0x9 => {
                    self.send_frame(0xA, &data)?;
                    continue;
                }
                0xA => continue,
                _ => {
                    acc.extend_from_slice(&data);
                    if fin {
                        return String::from_utf8(acc).map_err(|_| "收到无效文本帧".into());
                    }
                }
            }
        }
    }
}

fn ws_connect_url(url: &str) -> Result<WsConn, String> {
    let rest = url.strip_prefix("ws://").ok_or("无效的 WebSocket 地址")?;
    let (hostport, path) = rest.split_once('/').ok_or("无效的 WebSocket 地址")?;
    let port: u16 = hostport
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .ok_or("无效的调试端口")?;
    WsConn::connect(port, &format!("/{path}"))
}

fn http_req(port: u16, method: &str, path: &str) -> Result<String, String> {
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("连接 {port} 端口失败：{e}"))?;
    stream.set_read_timeout(Some(Duration::from_millis(1200))).ok();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("请求发送失败：{e}"))?;
    let mut raw: Vec<u8> = Vec::new();
    let mut b = [0u8; 4096];
    let deadline = std::time::Instant::now() + Duration::from_secs(6);
    loop {
        if let Some(sep) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head_str = String::from_utf8_lossy(&raw[..sep]).to_string();
            let head_lower = head_str.to_ascii_lowercase();
            let content_length = head_str.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                if k.trim().eq_ignore_ascii_case("content-length") {
                    v.trim().parse::<usize>().ok()
                } else {
                    None
                }
            });
            let chunked =
                head_lower.contains("transfer-encoding:") && head_lower.contains("chunked");
            if let Some(n) = content_length {
                if raw.len() >= sep + 4 + n {
                    break;
                }
            } else if chunked && raw.ends_with(b"0\r\n\r\n") {
                break;
            }
        }
        if std::time::Instant::now() > deadline {
            break;
        }
        match stream.read(&mut b) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&b[..n]),
            Err(_) => continue,
        }
    }
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("响应缺少头部结束符")?;
    let head_str = String::from_utf8_lossy(&raw[..sep]).to_string();
    let status = head_str
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    let head_lower = head_str.to_ascii_lowercase();
    let content_length = head_str.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if k.trim().eq_ignore_ascii_case("content-length") {
            v.trim().parse::<usize>().ok()
        } else {
            None
        }
    });
    let mut body: Vec<u8> = raw[sep + 4..].to_vec();
    if let Some(n) = content_length {
        body.truncate(n);
    } else if head_lower.contains("transfer-encoding:") && head_lower.contains("chunked") {
        body = dechunk(&body)?;
    }
    if !(200..300).contains(&status) {
        return Err(format!(
            "HTTP {status}：{}",
            String::from_utf8_lossy(&body[..body.len().min(120)])
        ));
    }
    Ok(String::from_utf8_lossy(&body).to_string())
}

fn dechunk(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let Some(rel) = data[i..].windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let line_end = i + rel;
        let size_str = String::from_utf8_lossy(&data[i..line_end]).to_string();
        let size = usize::from_str_radix(size_str.trim().split(';').next().unwrap_or("0"), 16)
            .map_err(|e| format!("chunk 长度解析失败：{e}"))?;
        i = line_end + 2;
        if size == 0 {
            break;
        }
        let end = (i + size).min(data.len());
        out.extend_from_slice(&data[i..end]);
        i = end + 2;
    }
    Ok(out)
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn cdp_ready() -> bool {
    is_cdp_alive(cdp_port())
}

fn cdp_call(
    ws: &mut WsConn,
    method: &str,
    params: Value,
    timeout_secs: u64,
) -> Result<Value, String> {
    ws.set_read_timeout(timeout_secs);
    let payload = serde_json::json!({ "id": 1, "method": method, "params": params });
    ws.send_text(&payload.to_string())?;
    loop {
        let msg = ws.recv_text()?;
        let v: Value =
            serde_json::from_str(&msg).map_err(|e| format!("CDP 响应解析失败：{e}"))?;
        if v.get("id").and_then(|x| x.as_i64()) == Some(1) {
            if let Some(err) = v.get("error") {
                return Err(format!(
                    "CDP 错误：{}",
                    err.get("message").and_then(|m| m.as_str()).unwrap_or("?")
                ));
            }
            return Ok(v);
        }
    }
}

fn cdp_eval(ws: &mut WsConn, expr: &str, timeout_secs: u64) -> Result<Value, String> {
    let r = cdp_call(
        ws,
        "Runtime.evaluate",
        serde_json::json!({ "expression": expr, "awaitPromise": true, "returnByValue": true }),
        timeout_secs,
    )?;
    if let Some(exc) = r.pointer("/result/exceptionDetails") {
        let d = exc
            .pointer("/exception/description")
            .and_then(|x| x.as_str())
            .or_else(|| exc.get("text").and_then(|x| x.as_str()))
            .unwrap_or("未知异常");
        return Err(format!("页面脚本执行失败：{d}"));
    }
    Ok(r)
}

static BRIDGE_WS: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn bridge_ready_js() -> &'static str {
    r#"(function(){try{return !!(window.chrome&&chrome.quarkWirelessSecurityGuardPrivate&&chrome.quarkAiChatPrivate&&chrome.quarkAiChatPrivate.sendToAgent&&chrome.cmsPrivate)}catch(e){return false}})()"#
}

fn qianwen_page_ws_urls() -> Result<Vec<String>, String> {
    let body = http_req(cdp_port(), "GET", "/json")?;
    let tabs: Vec<Value> = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    Ok(tabs
        .iter()
        .filter(|t| t.get("type").and_then(|x| x.as_str()) == Some("page"))
        .filter(|t| {
            t.get("url")
                .and_then(|x| x.as_str())
                .map(|u| u.contains("qianwen.com/quarkchat"))
                .unwrap_or(false)
        })
        .filter_map(|t| {
            t.get("webSocketDebuggerUrl")
                .and_then(|x| x.as_str())
                .map(String::from)
        })
        .collect())
}

fn try_bridge_eval(ws_url: &str, js: &str, timeout_secs: u64) -> Result<Value, String> {
    let mut ws = ws_connect_url(ws_url)?;
    let r = cdp_eval(&mut ws, js, timeout_secs)?;
    Ok(r.pointer("/result/result/value").cloned().unwrap_or(Value::Null))
}

fn ensure_bridge_ws(refresh: bool) -> Result<String, String> {
    if !refresh {
        let cached = BRIDGE_WS.lock().map_err(|e| e.to_string())?.clone();
        if let Some(ws) = cached {
            if try_bridge_eval(&ws, "1", 3).is_ok() {
                return Ok(ws);
            }
            dbg_log("bridge: cached target dead, repicking");
            if let Ok(mut g) = BRIDGE_WS.lock() {
                *g = None;
            }
        }
    } else if let Ok(mut g) = BRIDGE_WS.lock() {
        *g = None;
    }
    if let Ok(urls) = qianwen_page_ws_urls() {
        for u in urls {
            if let Ok(v) = try_bridge_eval(&u, bridge_ready_js(), 4) {
                if v.as_bool().unwrap_or(false) {
                    if let Ok(mut g) = BRIDGE_WS.lock() {
                        *g = Some(u.clone());
                    }
                    dbg_log("bridge: reusing existing quarkchat page");
                    return Ok(u);
                }
            }
        }
    }
    let body = http_req(cdp_port(), "PUT", "/json/new")?;
    let tab: Value =
        serde_json::from_str(&body).map_err(|e| format!("新标签页响应解析失败：{e}"))?;
    let ws_url = tab
        .get("webSocketDebuggerUrl")
        .and_then(|x| x.as_str())
        .ok_or("未取到调试 WebSocket 地址")?
        .to_string();
    {
        let mut ws = ws_connect_url(&ws_url)?;
        cdp_call(
            &mut ws,
            "Page.navigate",
            serde_json::json!({ "url": config::CHAT_URL }),
            30,
        )?;
    }
    let mut ready = false;
    for _ in 0..50 {
        thread::sleep(Duration::from_millis(500));
        if let Ok(v) = try_bridge_eval(&ws_url, bridge_ready_js(), 4) {
            if v.as_bool().unwrap_or(false) {
                ready = true;
                break;
            }
        }
    }
    if !ready {
        return Err("桥标签页就绪超时（25 秒）".into());
    }
    if let Ok(mut g) = BRIDGE_WS.lock() {
        *g = Some(ws_url.clone());
    }
    dbg_log("bridge: dedicated chat tab ready");
    Ok(ws_url)
}

fn bridge_eval(js: &str, timeout_secs: u64) -> Result<Value, String> {
    let mut last_err = String::from("真实浏览器桥不可用");
    for attempt in 0..2 {
        let ws_url = match ensure_bridge_ws(attempt > 0) {
            Ok(u) => u,
            Err(e) => {
                dbg_log(&format!("bridge: ensure fail (attempt {}): {e}", attempt + 1));
                last_err = e;
                continue;
            }
        };
        match try_bridge_eval(&ws_url, js, timeout_secs) {
            Ok(v) => return Ok(v),
            Err(e) => {
                last_err = e.clone();
                dbg_log(&format!("bridge: eval fail (attempt {}): {e}", attempt + 1));
            }
        }
    }
    Err(last_err)
}

const AGENT_HOOK_JS: &str = r#"
(function(){
  try {
    if (!window.chrome || !chrome.quarkAiChatPrivate || !chrome.quarkAiChatPrivate.onAgentMessage) return 'nobridge';
    if (!window.__qrAgentOut) window.__qrAgentOut = [];
    if (window.__qrAgentListener) { try { chrome.quarkAiChatPrivate.onAgentMessage.removeListener(window.__qrAgentListener); } catch(e){} }
    window.__qrAgentListener = function(m){
      try { if (window.__qrAgentOut.length < 200) window.__qrAgentOut.push(m); } catch(e){}
    };
    chrome.quarkAiChatPrivate.onAgentMessage.addListener(window.__qrAgentListener);
    window.__qrAgentHooked = 1;
    return 'ok';
  } catch(e) { return 'err:' + e; }
})()
"#;

pub fn bridge_call(api: &str, args: &[Value]) -> Result<Value, String> {
    match api {
        "sign" => {
            let text = args.first().and_then(|v| v.as_str()).unwrap_or("");
            let tag = args.get(1).and_then(|v| v.as_str()).unwrap_or("");
            let t = serde_json::to_string(text).unwrap_or_default();
            let g = serde_json::to_string(tag).unwrap_or_default();
            let js = format!(
                r#"(async function(){{try{{var t={t},g={g};return await new Promise(function(r){{try{{chrome.quarkWirelessSecurityGuardPrivate.sign(t,g,function(v){{r(v===undefined?null:v)}})}}catch(e){{r(null)}}}})}}catch(e){{return null}}}})()"#
            );
            bridge_eval(&js, 8)
        }
        "cms" => {
            let arg = args.first().cloned().unwrap_or(Value::Null);
            let c = serde_json::to_string(&arg).unwrap_or_else(|_| "null".into());
            let js = format!(
                r#"(async function(){{try{{var c={c};return await new Promise(function(r){{try{{var p=chrome.cmsPrivate.getAvailableCMSResource(c,function(v){{r(v===undefined?null:v)}});if(p&&typeof p.then==='function')p.then(function(v){{r(v===undefined?null:v)}},function(){{r(null)}})}}catch(e){{r(null)}}}})}}catch(e){{return null}}}})()"#
            );
            bridge_eval(&js, 8)
        }
        "agentSend" => {
            let msg = serde_json::to_string(args.first().unwrap_or(&Value::Null)).unwrap_or_default();
            dbg_log(&format!("bridge agentSend msg: {}", &msg[..msg.len().min(300)]));
            let js = format!(
                r#"(function(){{try{{var m={msg};if(!window.chrome||!chrome.quarkAiChatPrivate||!chrome.quarkAiChatPrivate.sendToAgent)return 'nobridge';{hook};chrome.quarkAiChatPrivate.sendToAgent(m);return 'ok'}}catch(e){{return 'err:'+e}}}})()"#,
                hook = AGENT_HOOK_JS
            );
            let v = bridge_eval(&js, 8)?;
            let s = v.as_str().unwrap_or("").to_string();
            dbg_log(&format!("bridge agentSend -> {s}"));
            if s == "ok" {
                if let Some(id) = args
                    .first()
                    .and_then(|a| a.get("id"))
                    .and_then(|x| x.as_str())
                {
                    if let Ok(mut ids) = AGENT_IDS.lock() {
                        if ids.len() < 1000 {
                            ids.insert(id.to_string());
                        }
                    }
                }
                Ok(Value::String("ok".into()))
            } else {
                Err(format!("agentSend 失败：{s}"))
            }
        }
        _ => Err(format!("未知桥 API：{api}")),
    }
}

pub fn start_agent_poll() {
    dbg_log("agent_poll: started");
    let mut last_err = String::new();
    let mut quiet_ticks: u32 = 0;
    let mut ready_fails: u32 = 0;
    let mut ticks: u32 = 0;
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(400));
        if !cdp_ready() {
            ready_fails += 1;
            if ready_fails % 25 == 1 {
                dbg_log(&format!("agent_poll: cdp_ready false x{ready_fails}"));
            }
            continue;
        }
        ready_fails = 0;
        ticks += 1;
        let js = r#"(function(){try{if(!window.__qrAgentOut)window.__qrAgentOut=[];if(window.__qrAgentListener){try{chrome.quarkAiChatPrivate.onAgentMessage.removeListener(window.__qrAgentListener)}catch(e){}}window.__qrAgentListener=function(m){try{if(window.__qrAgentOut.length<200)window.__qrAgentOut.push(m)}catch(e){}};try{chrome.quarkAiChatPrivate.onAgentMessage.addListener(window.__qrAgentListener)}catch(e){}var q=window.__qrAgentOut;window.__qrAgentOut=[];return JSON.stringify({href:location.href.slice(0,60),hooked:window.__qrAgentHooked||0,q:q})}catch(e){return '{"q":[]}'}})()"#;
        let v = match bridge_eval(js, 5) {
            Ok(v) => {
                last_err.clear();
                v
            }
            Err(e) => {
                if e != last_err {
                    dbg_log(&format!("agent_poll eval err: {e}"));
                    last_err = e;
                }
                continue;
            }
        };
        let state: Value = match serde_json::from_str(v.as_str().unwrap_or("{}")) {
            Ok(s) => s,
            Err(e) => {
                if ticks % 10 == 1 {
                    dbg_log(&format!("agent_poll tick#{ticks}: parse fail {e}"));
                }
                continue;
            }
        };
        let list = state
            .get("q")
            .and_then(|q| q.as_array().cloned())
            .unwrap_or_default();
        if list.is_empty() {
            quiet_ticks += 1;
            if quiet_ticks % 25 == 0 {
                dbg_log(&format!(
                    "agent_poll alive x{}: href={} hooked={}",
                    quiet_ticks,
                    state.get("href").and_then(|x| x.as_str()).unwrap_or("?"),
                    state.get("hooked").and_then(|x| x.as_i64()).unwrap_or(-1)
                ));
            }
            continue;
        }
        quiet_ticks = 0;
        dbg_log(&format!(
            "agent_poll: {} msgs (href={})",
            list.len(),
            state.get("href").and_then(|x| x.as_str()).unwrap_or("?")
        ));
        for m in list {
            let mid = m.get("id").and_then(|x| x.as_str()).map(String::from);
            let brief = {
                let id = mid.as_deref().unwrap_or("-");
                let method = m.get("method").and_then(|x| x.as_str()).unwrap_or("-");
                let kind = m.get("kind").and_then(|x| x.as_str()).unwrap_or("-");
                let rkeys = m
                    .get("result")
                    .map(|r| {
                        r.as_object()
                            .map(|o| o.keys().cloned().collect::<Vec<_>>().join(","))
                            .unwrap_or_else(|| "nonobj".into())
                    })
                    .unwrap_or_else(|| "-".into());
                let err = m
                    .get("error")
                    .map(|e| serde_json::to_string(e).unwrap_or_default())
                    .unwrap_or_default();
                format!("agent_msg id={id} m={method} k={kind} res=[{rkeys}] err={err}")
            };
            if let Some(id) = &mid {
                let known = AGENT_IDS
                    .lock()
                    .map(|mut s| s.remove(id))
                    .unwrap_or(false);
                if !known {
                    dbg_log(&format!("{brief} [DROPPED]"));
                    continue;
                }
            }
            dbg_log(&format!("{brief} [PASS]"));
            if let Ok(mut q) = AGENT_QUEUE.lock() {
                if q.len() < 500 {
                    q.push(m);
                }
            }
        }
    });
}

static AGENT_QUEUE: std::sync::Mutex<Vec<Value>> = std::sync::Mutex::new(Vec::new());
static AGENT_IDS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

pub fn take_agent_queue() -> Value {
    match AGENT_QUEUE.lock() {
        Ok(mut q) => {
            let out: Vec<Value> = std::mem::take(&mut *q);
            serde_json::json!(out)
        }
        Err(_) => serde_json::json!([]),
    }
}


pub fn start_grab(proxy: tao::event_loop::EventLoopProxy<crate::AppEvent>) {
    thread::spawn(move || {
        let (ok, msg) = match std::env::var("QR_TRACE").as_deref() {
            Ok("2") => network_trace().unwrap_or_else(|e| (false, e)),
            Ok("1") => trace_flow().unwrap_or_else(|e| (false, e)),
            _ => match grab_flow() {
                Ok(pair) => pair,
                Err(e) => (false, e),
            },
        };
        let _ = proxy.send_event(crate::AppEvent::CredsReported(ok, msg));
    });
}

fn open_chat_tab() -> Result<(String, WsConn), String> {
    launch_cdp_quark()?;
    let new_tab = http_req(
        cdp_port(),
        "PUT",
        &format!("/json/new?url={}", urlencode("about:blank")),
    )?;
    let tab: Value =
        serde_json::from_str(&new_tab).map_err(|e| format!("新标签页响应解析失败：{e}"))?;
    let target_id = tab
        .get("id")
        .and_then(|x| x.as_str())
        .ok_or("未取到标签页 ID")?
        .to_string();
    let ws_url = tab
        .get("webSocketDebuggerUrl")
        .and_then(|x| x.as_str())
        .ok_or("未取到调试 WebSocket 地址")?
        .to_string();
    let ws = ws_connect_url(&ws_url)?;
    Ok((target_id, ws))
}

fn trace_flow() -> Result<(bool, String), String> {
    dbg_log("trace_flow start");
    let (target_id, mut ws) = open_chat_tab()?;
    let add = cdp_call(
        &mut ws,
        "Page.addScriptToEvaluateOnNewDocument",
        serde_json::json!({ "source": TRACE_HOOK_JS }),
        15,
    );
    dbg_log(&format!("trace: addScript resp: {}", serde_json::to_string(&add).unwrap_or_default()));
    cdp_call(
        &mut ws,
        "Page.navigate",
        serde_json::json!({ "url": config::CHAT_URL }),
        30,
    )?;
    dbg_log("trace: navigated, injecting hook immediately");
    for i in 0..6 {
        thread::sleep(Duration::from_millis(300));
        match cdp_eval(&mut ws, TRACE_HOOK_JS, 10) {
            Ok(_) => {
                dbg_log(&format!("trace: hook injected at try {i}"));
                break;
            }
            Err(e) => dbg_log(&format!("trace: inject try {i} err: {e}")),
        }
    }
    dbg_log("trace: waiting 25s");
    thread::sleep(Duration::from_secs(25));
    let resp = cdp_eval(
        &mut ws,
        "JSON.stringify({href:location.href.slice(0,60), inst:!!window.__qrTraceInstalled, ucapi:!!window.ucapi, chrome:!!window.chrome, chromeKeys:window.chrome?Object.keys(window.chrome).length:-1, log:(window.__qrTraceLog||['NOLOG']).slice(0,400)})",
        30,
    )?;
    let val = resp
        .pointer("/result/result/value")
        .and_then(|x| x.as_str())
        .unwrap_or("[]");
    let _ = std::fs::write(config::app_dir().join("trace-log.json"), val);
    dbg_log(&format!("trace: {} bytes written", val.len()));
    let _ = http_req(cdp_port(), "GET", &format!("/json/close/{target_id}"));
    Ok((false, "TRACE_DONE".into()))
}

fn network_trace() -> Result<(bool, String), String> {
    dbg_log("network_trace start");
    let (target_id, mut ws) = open_chat_tab()?;
    cdp_call(&mut ws, "Network.enable", serde_json::json!({}), 15)?;
    dbg_log("network_trace: Network enabled, navigating");
    cdp_call(
        &mut ws,
        "Page.navigate",
        serde_json::json!({ "url": config::CHAT_URL }),
        30,
    )?;
    ws.set_read_timeout(3);
    let t0 = std::time::Instant::now();
    let mut requests: Vec<Value> = Vec::new();
    let mut extras: Vec<Value> = Vec::new();
    while t0.elapsed() < Duration::from_secs(45) {
        let msg = match ws.recv_text() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let Ok(v) = serde_json::from_str::<Value>(&msg) else {
            continue;
        };
        let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
        match method {
            "Network.requestWillBeSent" => {
                let url = v
                    .pointer("/params/request/url")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                if url.contains("chat-api")
                    || url.contains("qianwen.com")
                    || url.contains("quark")
                    || url.contains("sign")
                {
                    requests.push(serde_json::json!({
                        "id": v.pointer("/params/requestId").cloned().unwrap_or(Value::Null),
                        "url": url,
                        "method": v.pointer("/params/request/method").cloned().unwrap_or(Value::Null),
                        "postData": v.pointer("/params/request/postData").cloned().unwrap_or(Value::Null),
                        "headers": v.pointer("/params/request/headers").cloned().unwrap_or(Value::Null),
                    }));
                }
            }
            "Network.requestWillBeSentExtraInfo" => {
                let headers = v.pointer("/params/headers").cloned().unwrap_or(Value::Null);
                if !headers.is_null() {
                    extras.push(serde_json::json!({
                        "id": v.pointer("/params/requestId").cloned().unwrap_or(Value::Null),
                        "headers": headers,
                    }));
                }
            }
            _ => {}
        }
    }
    dbg_log(&format!(
        "network_trace: collected {} requests, {} extras",
        requests.len(),
        extras.len()
    ));
    let out = serde_json::json!({ "requests": requests, "extras": extras });
    let _ = std::fs::write(
        config::app_dir().join("network-log.json"),
        serde_json::to_string_pretty(&out).unwrap_or_default(),
    );
    let _ = http_req(cdp_port(), "GET", &format!("/json/close/{target_id}"));
    dbg_log("network_trace done");
    Ok((false, "NETTRACE_DONE".into()))
}

const TRACE_HOOK_JS: &str = r#"
(function(){
  if (window.__qrTraceInstalled) return;
  window.__qrTraceInstalled = 1;
  var LOG = window.__qrTraceLog = [];
  function log(s){ try{ if (LOG.length < 500) LOG.push(s); }catch(e){} }
  function sv(v){
    try {
      if (v == null) return String(v);
      if (typeof v === 'function') return 'fn';
      if (typeof v !== 'object') return String(v).slice(0, 80);
      var s = JSON.stringify(v);
      return s ? s.slice(0, 200) : '[obj]';
    } catch(e){ return '[?]'; }
  }
  function sa(a){
    try {
      return Array.prototype.slice.call(a).map(function(x){
        if (typeof x === 'function') return 'cb';
        return sv(x);
      }).join(',').slice(0, 120);
    } catch(e){ return '[?]'; }
  }
  var wrapped = new WeakSet();
  function wrapAny(path, target, depth){
    if (!target || (typeof target !== 'object' && typeof target !== 'function')) return target;
    if (wrapped.has(target)) return target;
    try { wrapped.add(target); } catch(e){}
    return new Proxy(target, {
      get: function(t, k){
        if (k === 'then' || k === 'catch' || k === 'finally' || k === 'prototype' || k === 'constructor') return undefined;
        var v;
        try { v = t[k]; } catch(e){ return undefined; }
        var p = path + '.' + String(k);
        if (typeof v === 'function') {
          var fn = v;
          return function(){
            var args = Array.prototype.slice.call(arguments);
            for (var i = 0; i < args.length; i++) {
              (function(idx){
                if (typeof args[idx] === 'function') {
                  var of = args[idx];
                  args[idx] = function(){
                    log(p + ' CB(' + idx + ')=' + sv(arguments[0]));
                    return of.apply(this, arguments);
                  };
                }
              })(i);
            }
            var r = fn.apply(t, args);
            var ps = p + '(' + sa(arguments) + ')';
            if (r && typeof r.then === 'function') {
              r.then(function(rv){ log(ps + ' -> ' + sv(rv)); }, function(e){ log(ps + ' REJ ' + sv(e)); });
            } else {
              log(ps + ' -> ' + sv(r));
            }
            return r;
          };
        }
        if (v && typeof v === 'object' && depth < 3) return wrapAny(p, v, depth + 1);
        if (v === undefined) log(p + ' = undefined');
        return v;
      }
    });
  }
  try { if (window.ucapi) { window.ucapi = wrapAny('ucapi', window.ucapi, 0); log('ucapi hooked'); } else log('no ucapi at start'); } catch(e){ log('ucapi hook err ' + e); }
  try {
    if (window.chrome) {
      var _c = window.chrome;
      var _w = wrapAny('chrome', _c, 0);
      try {
        Object.defineProperty(window, 'chrome', { configurable: true, get: function(){ return _w; }, set: function(v){ _w = wrapAny('chrome', v, 0); log('chrome re-set'); } });
      } catch(e){ window.chrome = _w; }
      log('chrome hooked');
    } else {
      var _v = undefined;
      Object.defineProperty(window, 'chrome', { configurable: true, get: function(){ return _v; }, set: function(v){ _v = wrapAny('chrome', v, 0); log('chrome SET late'); } });
      log('no chrome at start, watching');
    }
  } catch(e){ log('chrome hook err ' + e); }
})();
"#;

const QUARK_NOT_FOUND_MSG: &str = "未找到夸克浏览器。请先安装夸克 PC 浏览器；\
若已安装在自定义目录，请在小窗设置中填写 quark.exe 的完整路径。";

fn quark_running() -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq quark.exe", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .to_lowercase()
                .contains("quark.exe")
        })
        .unwrap_or(false)
}

fn quark_user_data_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Quark").join("User Data"));
    }
    if let Some(exe) = quark_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("User Data"));
        }
    }
    candidates.into_iter().find(|ud| {
        ud.join("Local State").is_file()
            && (ud.join("Default").join("Network").join("Cookies").is_file()
                || ud.join("Default").join("Cookies").is_file())
    })
}

fn copy_if_exists(src: &std::path::Path, dst: &std::path::Path) {
    if !src.is_file() {
        return;
    }
    if let Some(parent) = dst.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::copy(src, dst);
}

fn cdp_profile_dir() -> PathBuf {
    config::app_dir().join("cdp-profile")
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path, max_file: u64) {
    let Ok(entries) = std::fs::read_dir(src) else {
        return;
    };
    let _ = std::fs::create_dir_all(dst);
    for e in entries.filter_map(|x| x.ok()) {
        let s = e.path();
        let d = dst.join(e.file_name());
        if s.is_dir() {
            copy_dir_recursive(&s, &d, max_file);
        } else if let Ok(m) = e.metadata() {
            if m.len() <= max_file {
                let _ = std::fs::copy(&s, &d);
            }
        }
    }
}

fn clone_profile_for_cdp() -> Option<PathBuf> {
    let src = quark_user_data_dir()?;
    let dst = cdp_profile_dir();
    const CLONE_VERSION: &str = "2";
    let ver_ok =
        std::fs::read_to_string(dst.join(".qr-clone-ver")).unwrap_or_default() == CLONE_VERSION;
    let user_info_ok = dst.join("user_info").is_dir();
    if ver_ok && user_info_ok {
        return Some(dst);
    }
    dbg_log(&format!(
        "cloning profile {} -> {} (full, ver {CLONE_VERSION})",
        src.display(),
        dst.display()
    ));
    let _ = std::fs::remove_dir_all(&dst);
    const MAX_FILE: u64 = 20 * 1024 * 1024;
    copy_if_exists(&src.join("Local State"), &dst.join("Local State"));
    copy_dir_recursive(&src.join("user_info"), &dst.join("user_info"), MAX_FILE);
    let sdef = src.join("Default");
    let ddef = dst.join("Default");
    let mut cookies_copied = false;
    if let Ok(entries) = std::fs::read_dir(&sdef) {
        for e in entries.filter_map(|x| x.ok()) {
            let s = e.path();
            let d = ddef.join(e.file_name());
            if s.is_dir() {
                continue;
            }
            if let Ok(m) = e.metadata() {
                if m.len() <= MAX_FILE && std::fs::copy(&s, &d).is_ok() {
                    if e.file_name().eq_ignore_ascii_case("Cookies") {
                        cookies_copied = true;
                    }
                }
            }
        }
    }
    for dir in [
        "Network",
        "quark_leveldb_storage",
        "Local Storage",
        "Session Storage",
        "WebStorage",
        "Sessions",
        "Sync Data",
        "Local Extension Settings",
        "Sync Extension Settings",
    ] {
        let s = sdef.join(dir);
        if s.is_dir() {
            copy_dir_recursive(&s, &ddef.join(dir), MAX_FILE);
        }
    }
    if !dst.join("user_info").is_dir() {
        dbg_log("clone failed: user_info not copied");
        return None;
    }
    if !cookies_copied && !dst.join("Default").join("Network").join("Cookies").is_file() {
        dbg_log("Cookies locked or absent (browser running); isolated instance will rebuild or wait for login");
    }
    let _ = std::fs::write(dst.join(".qr-clone-ver"), CLONE_VERSION);
    dbg_log("profile cloned");
    Some(dst)
}

static OWN_CDP_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn stop_own_cdp_instance() {
    let pid = OWN_CDP_PID.swap(0, std::sync::atomic::Ordering::SeqCst);
    if pid == 0 {
        return;
    }
    dbg_log(&format!("stopping own cdp instance pid={pid} gracefully"));
    let mut closed = false;
    if let Ok(ver) = http_req(cdp_port(), "GET", "/json/version") {
        if let Ok(v) = serde_json::from_str::<Value>(&ver) {
            if let Some(ws_url) = v.get("webSocketDebuggerUrl").and_then(|x| x.as_str()) {
                if let Ok(mut ws) = ws_connect_url(ws_url) {
                    if cdp_call(&mut ws, "Browser.close", serde_json::json!({}), 10).is_ok() {
                        closed = true;
                    }
                }
            }
        }
    }
    if closed {
        for _ in 0..10 {
            thread::sleep(Duration::from_millis(500));
            if !cdp_ready() {
                dbg_log("own instance closed gracefully");
                return;
            }
        }
    }
    dbg_log("graceful close failed/timeout, force killing");
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

fn launch_cdp_quark() -> Result<(), String> {
    if cdp_ready() {
        return Ok(());
    }
    let exe = quark_exe().ok_or(QUARK_NOT_FOUND_MSG)?;
    let port = cdp_port();
    let running = quark_running();
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg(format!("--remote-debugging-port={port}"))
        .arg("--no-first-run")
        .arg("--no-default-browser-check");
    if running {
        let dir = clone_profile_for_cdp();
        if let Some(d) = &dir {
            dbg_log(&format!(
                "quark running, launching isolated instance with profile {}",
                d.display()
            ));
            cmd.arg(format!("--user-data-dir={}", d.display()));
        } else {
            dbg_log("quark running but no profile cloned, isolated profile will be fresh");
            cmd.arg(format!("--user-data-dir={}", cdp_profile_dir().display()));
        }
    } else {
        dbg_log("quark not running, launching with user profile + debug port");
    }
    let child = cmd
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("启动夸克浏览器失败：{e}"))?;
    if running {
        OWN_CDP_PID.store(child.id(), std::sync::atomic::Ordering::SeqCst);
    }
    let mut up = false;
    for i in 0..40 {
        thread::sleep(Duration::from_millis(500));
        if cdp_ready() {
            up = true;
            break;
        }
        if i % 10 == 9 {
            dbg_log("still waiting for debug port");
        }
    }
    if !up {
        dbg_log("debug port timeout (20s)");
        stop_own_cdp_instance();
        return Err(format!(
            "夸克浏览器调试端口 {port} 未就绪（20 秒超时），请重试。"
        ));
    }
    dbg_log("debug port ready");
    Ok(())
}

fn quark_exe() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let custom = config::load().quark_exe.trim().to_string();
    if !custom.is_empty() {
        candidates.push(PathBuf::from(&custom));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Programs").join("Quark").join("quark.exe"));
    }
    candidates.push(PathBuf::from(r"C:\Program Files\Quark\quark.exe"));
    candidates.push(PathBuf::from(r"C:\Program Files (x86)\Quark\quark.exe"));
    candidates.extend(quark_exe_from_registry());
    candidates.into_iter().find(|p| p.is_file())
}

fn quark_exe_from_registry() -> Vec<PathBuf> {
    use windows_registry::{CURRENT_USER, LOCAL_MACHINE};
    let mut out: Vec<PathBuf> = Vec::new();
    let roots = [CURRENT_USER, LOCAL_MACHINE];

    for root in roots {
        for sub in [
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\quark.exe",
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\quark.exe",
        ] {
            if let Ok(key) = root.open(sub) {
                if let Ok(path) = key.get_string("") {
                    let p = PathBuf::from(path.trim());
                    if p.is_file() {
                        out.push(p);
                    }
                }
            }
        }
    }

    for root in roots {
        for sub in [
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ] {
            let Ok(key) = root.open(sub) else { continue };
            let Ok(names) = key.keys() else { continue };
            for name in names {
                let Ok(sk) = key.open(&name) else { continue };
                let display = sk.get_string("DisplayName").unwrap_or_default();
                let lower = display.to_lowercase();
                if (!lower.contains("quark") && !display.contains("夸克")) || lower.contains("reflex")
                {
                    continue;
                }
                if let Ok(loc) = sk.get_string("InstallLocation") {
                    let loc = loc.trim();
                    if !loc.is_empty() {
                        out.push(PathBuf::from(loc).join("quark.exe"));
                    }
                }
                for field in ["DisplayIcon", "UninstallString"] {
                    if let Ok(raw) = sk.get_string(field) {
                        push_quark_candidates(&mut out, &raw);
                    }
                }
            }
        }
    }
    out
}

fn push_quark_candidates(out: &mut Vec<PathBuf>, raw: &str) {
    let s = raw.trim();
    if s.is_empty() {
        return;
    }
    let s = s.split(',').next().unwrap_or("").trim();
    let paths: Vec<String> = if let Some(rest) = s.strip_prefix('"') {
        match rest.find('"') {
            Some(end) => vec![rest[..end].to_string()],
            None => vec![rest.to_string()],
        }
    } else {
        let words: Vec<&str> = s.split(' ').collect();
        (1..=words.len())
            .map(|i| words[..i].join(" "))
            .collect()
    };
    for p in paths {
        let pb = PathBuf::from(p.trim());
        if pb.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false) {
            let is_quark = pb
                .file_name()
                .map(|f| f.to_string_lossy().eq_ignore_ascii_case("quark.exe"))
                .unwrap_or(false);
            if is_quark {
                out.push(pb);
            } else if let Some(dir) = pb.parent() {
                out.push(dir.join("quark.exe"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn quark_registry_probe_finds_local_install() {
        let found = super::quark_exe_from_registry();
        println!("registry candidates: {found:#?}");
        let exe = super::quark_exe();
        println!("resolved: {exe:?}");
        assert!(exe.is_some(), "本机已安装夸克，应能解析出 quark.exe 路径");
        let exe = exe.unwrap();
        assert!(
            exe.file_name()
                .map(|f| f.to_string_lossy().eq_ignore_ascii_case("quark.exe"))
                .unwrap_or(false),
            "解析结果必须是 quark.exe：{exe:?}"
        );
    }

    #[test]
    fn cdp_port_helpers() {
        let p = (super::CDP_PORT_BASE..super::CDP_PORT_BASE + 20)
            .find(|p| super::port_free(*p))
            .expect("9333..9352 中应存在空闲端口");
        assert!(!super::is_cdp_alive(p), "空闲端口上不应有 CDP 服务");
    }

    #[test]
    fn quark_user_data_dir_found_on_local_machine() {
        let ud = super::quark_user_data_dir();
        println!("user data dir: {ud:?}");
        assert!(ud.is_some(), "本机装有夸克浏览器，应能找到 User Data 目录");
        let ud = ud.unwrap();
        assert!(ud.join("Local State").is_file());
    }

    #[test]
    fn copy_if_exists_roundtrip() {
        let tmp = std::env::temp_dir().join("qr-copy-test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src").join("Default").join("Network")).unwrap();
        std::fs::create_dir_all(tmp.join("dst").join("Default").join("Network")).unwrap();
        let src_file = tmp.join("src").join("Default").join("Network").join("Cookies");
        std::fs::write(&src_file, b"cookie-blob").unwrap();
        super::copy_if_exists(
            &src_file,
            &tmp.join("dst").join("Default").join("Network").join("Cookies"),
        );
        let dst_file = tmp.join("dst").join("Default").join("Network").join("Cookies");
        assert_eq!(std::fs::read(&dst_file).unwrap(), b"cookie-blob");
        super::copy_if_exists(&tmp.join("src").join("nope"), &tmp.join("dst").join("nope"));
        assert!(!tmp.join("dst").join("nope").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn clone_profile_copies_login_state_files() {
        let _ = std::fs::remove_dir_all(super::cdp_profile_dir());
        let cloned = super::clone_profile_for_cdp();
        println!("cloned: {cloned:?}");
        let dst = super::cdp_profile_dir();
        assert!(cloned.is_some());
        assert!(
            dst.join("user_info").is_dir(),
            "user_info 目录（夸克账号登录态，无文件锁）必须复制成功"
        );
        assert!(dst.join("Local State").is_file());
        let cookies = dst.join("Default").join("Network").join("Cookies");
        println!(
            "cookies copied: {} (夸克运行中会被独占锁定，允许缺失)",
            cookies.is_file()
        );
        assert_eq!(
            std::fs::read_to_string(dst.join(".qr-clone-ver")).unwrap_or_default(),
            "2"
        );
    }

    #[test]
    fn quark_candidates_from_cmdline() {
        let mut out = Vec::new();
        super::push_quark_candidates(&mut out, r#""D:\My Apps\Quark\Uninstall.exe" --mode"#);
        assert!(out.contains(&std::path::PathBuf::from(r"D:\My Apps\Quark\quark.exe")));

        let mut out = Vec::new();
        super::push_quark_candidates(&mut out, r"E:\Soft\Quark\quark.exe,0");
        assert!(out.contains(&std::path::PathBuf::from(r"E:\Soft\Quark\quark.exe")));

        let mut out = Vec::new();
        super::push_quark_candidates(&mut out, "");
        assert!(out.is_empty());
    }
}

fn grab_flow() -> Result<(bool, String), String> {
    let r = grab_flow_inner();
    stop_own_cdp_instance();
    r
}

fn grab_flow_inner() -> Result<(bool, String), String> {
    dbg_log("grab_flow start");
    launch_cdp_quark()?;

    dbg_log("opening new tab");
    let new_tab = http_req(
        cdp_port(),
        "PUT",
        &format!("/json/new?url={}", urlencode(config::CHAT_URL)),
    )?;
    let tab: Value =
        serde_json::from_str(&new_tab).map_err(|e| format!("新标签页响应解析失败：{e}"))?;
    let target_id = tab
        .get("id")
        .and_then(|x| x.as_str())
        .ok_or("未取到标签页 ID")?
        .to_string();
    let ws_url = tab
        .get("webSocketDebuggerUrl")
        .and_then(|x| x.as_str())
        .ok_or("未取到调试 WebSocket 地址")?
        .to_string();
    dbg_log(&format!("tab {target_id} opened"));

    let mut ws = ws_connect_url(&ws_url)?;
    dbg_log("navigating tab");
    let nav = cdp_call(
        &mut ws,
        "Page.navigate",
        serde_json::json!({ "url": config::CHAT_URL }),
        30,
    )?;
    dbg_log(&format!(
        "navigate resp: {}",
        nav.pointer("/result/frameId")
            .and_then(|x| x.as_str())
            .unwrap_or("?")
    ));
    let mut href = String::new();
    for i in 0..20 {
        thread::sleep(Duration::from_millis(1000));
        match cdp_eval(&mut ws, "location.href", 10) {
            Ok(resp) => {
                href = resp
                    .pointer("/result/result/value")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                dbg_log(&format!("poll {i}: href={href}"));
                if href.starts_with("https://www.qianwen.com") {
                    break;
                }
            }
            Err(e) => {
                dbg_log(&format!("poll {i}: eval err: {e}"));
                ws = match ws_connect_url(&ws_url) {
                    Ok(w) => w,
                    Err(e) => return Err(e),
                };
            }
        }
    }
    if !href.starts_with("https://www.qianwen.com") {
        dbg_log("page never reached qianwen.com");
        let _ = http_req(cdp_port(), "GET", &format!("/json/close/{target_id}"));
        return Err("夸克聊天页在夸克浏览器中未能加载，请重试。".into());
    }

    dbg_log("running GRAB_EXPR");
    let resp = cdp_eval(&mut ws, GRAB_EXPR, 90)?;
    dbg_log("GRAB_EXPR returned");
    let val = resp
        .pointer("/result/result/value")
        .and_then(|x| x.as_str())
        .ok_or("抓取脚本未返回结果")?;
    let _ = std::fs::create_dir_all(config::app_dir());
    let mut report: Value =
        serde_json::from_str(val).map_err(|e| format!("抓取结果解析失败：{e}"))?;
    grab_cookies(&mut report);
    grab_net_params(&mut report);
    let _ = std::fs::write(
        config::app_dir().join("creds-report.json"),
        serde_json::to_string(&report).unwrap_or_else(|_| val.to_string()),
    );
    dbg_log(&format!("report written, {} bytes", val.len()));
    let (ok, msg) = apply_report(&report);

    let (ok, msg) = if !ok
        && msg.contains("kps_wg")
        && OWN_CDP_PID.load(std::sync::atomic::Ordering::SeqCst) != 0
    {
        dbg_log("kps_wg missing, keeping window open and waiting for login");
        let mut final_result = (ok, msg);
        for round in 1..=40u32 {
            thread::sleep(Duration::from_secs(15));
            dbg_log(&format!("login wait round {round}"));
            let resp = match cdp_eval(&mut ws, GRAB_EXPR, 90) {
                Ok(r) => r,
                Err(e) => {
                    dbg_log(&format!("login wait eval err: {e}, reconnecting"));
                    match ws_connect_url(&ws_url) {
                        Ok(w) => {
                            ws = w;
                            continue;
                        }
                        Err(e2) => {
                            dbg_log(&format!("reconnect failed: {e2}"));
                            break;
                        }
                    }
                }
            };
            let Some(nval) = resp
                .pointer("/result/result/value")
                .and_then(|x| x.as_str())
            else {
                continue;
            };
            let Ok(new_fields) = serde_json::from_str::<Value>(nval) else {
                continue;
            };
            if let Some(obj) = report.as_object_mut() {
                for (k, v) in new_fields.as_object().into_iter().flatten() {
                    obj.insert(k.clone(), v.clone());
                }
            }
            grab_cookies(&mut report);
            let (ok2, msg2) = apply_report(&report);
            if ok2 {
                let _ = std::fs::write(
                    config::app_dir().join("creds-report.json"),
                    serde_json::to_string(&report).unwrap_or_default(),
                );
                final_result = (ok2, msg2);
                dbg_log("login detected during wait, success");
                break;
            }
            final_result = (ok2, msg2);
        }
        if !final_result.0 {
            final_result = (
                false,
                format!(
                    "{}\n\n已保持独立夸克窗口打开等待登录（未影响你正在使用的浏览器）。\
                     请在弹出的夸克窗口中登录夸克账号后，再次获取登录凭证\
                     （登录一次即可，之后会被记住）。等待期间未检测到登录。",
                    final_result.1
                ),
            );
        }
        final_result
    } else {
        (ok, msg)
    };
    dbg_log(&format!("apply_report ok={ok}"));
    let _ = http_req(cdp_port(), "GET", &format!("/json/close/{target_id}"));
    dbg_log("tab closed, flow done");
    Ok((ok, msg))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn grab_net_params(report: &mut Value) {
    dbg_log("grab_net_params: opening tab");
    let Ok((target_id, mut ws)) = open_chat_tab() else {
        dbg_log("grab_net_params: open tab failed");
        return;
    };
    if cdp_call(&mut ws, "Network.enable", serde_json::json!({}), 15).is_err() {
        let _ = http_req(cdp_port(), "GET", &format!("/json/close/{target_id}"));
        dbg_log("grab_net_params: Network.enable failed");
        return;
    }
    if cdp_call(
        &mut ws,
        "Page.navigate",
        serde_json::json!({ "url": config::CHAT_URL }),
        30,
    )
    .is_err()
    {
        let _ = http_req(cdp_port(), "GET", &format!("/json/close/{target_id}"));
        dbg_log("grab_net_params: navigate failed");
        return;
    }
    ws.set_read_timeout(3);
    let t0 = std::time::Instant::now();
    let mut params: Option<Value> = None;
    while t0.elapsed() < Duration::from_secs(35) {
        let msg = match ws.recv_text() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let Ok(v) = serde_json::from_str::<Value>(&msg) else {
            continue;
        };
        if v.get("method").and_then(|m| m.as_str()) != Some("Network.requestWillBeSent") {
            continue;
        }
        let url = v
            .pointer("/params/request/url")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if !url.contains("chat-api") {
            continue;
        }
        let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
        let mut sign_wg = String::new();
        let mut vcode = String::new();
        let mut kps = String::new();
        for kv in query.split('&') {
            let (k, val) = kv.split_once('=').unwrap_or(("", ""));
            match k {
                "sign_wg" => sign_wg = percent_decode(val),
                "vcode" => vcode = percent_decode(val),
                "kps_wg" => kps = percent_decode(val),
                _ => {}
            }
        }
        dbg_log(&format!(
            "grab_net_params: hit chat-api url sign_wg={}.. vcode={} kps={}..",
            sign_wg.len(),
            vcode,
            if kps.len() > 12 { &kps[..12] } else { &kps }
        ));
        if !sign_wg.is_empty() && !vcode.is_empty() {
            params = Some(serde_json::json!({ "sign_wg": sign_wg, "vcode": vcode, "kps_wg": kps }));
            break;
        }
    }
    let _ = http_req(cdp_port(), "GET", &format!("/json/close/{target_id}"));
    if let Some(p) = params {
        report["net_params"] = p;
        dbg_log("grab_net_params: saved");
    } else {
        dbg_log("grab_net_params: no chat-api request captured");
    }
}

fn grab_account_full(report: &mut Value) {
    dbg_log("grab_account_full: trying quark first-party pages");
    for url in [
        "https://quark.sm.cn/",
        "https://www.quark.cn/",
        "https://home.quark.cn/",
    ] {
        let Ok((target_id, mut ws)) = open_chat_tab() else {
            dbg_log("grab_account_full: open tab failed");
            return;
        };
        if cdp_call(
            &mut ws,
            "Page.navigate",
            serde_json::json!({ "url": url }),
            30,
        )
        .is_err()
        {
            let _ = http_req(cdp_port(), "GET", &format!("/json/close/{target_id}"));
            continue;
        }
        thread::sleep(Duration::from_millis(8000));
        let resp = match cdp_eval(&mut ws, ACCOUNT_EXPR, 40) {
            Ok(r) => r,
            Err(e) => {
                dbg_log(&format!("grab_account_full eval failed on {url}: {e}"));
                let _ = http_req(cdp_port(), "GET", &format!("/json/close/{target_id}"));
                continue;
            }
        };
        let val = resp
            .pointer("/result/result/value")
            .and_then(|x| x.as_str())
            .unwrap_or("{}");
        dbg_log(&format!("grab_account_full on {url}: {} bytes", val.len()));
        let _ = http_req(cdp_port(), "GET", &format!("/json/close/{target_id}"));
        if let Ok(v) = serde_json::from_str::<Value>(val) {
            if v.get("full").is_some() {
                dbg_log(&format!("grab_account_full: FULL found on {url}"));
                report["account_full"] = v;
                return;
            }
            let has_sign = v
                .get("results")
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter().any(|x| {
                        x.get("sign_wg").and_then(|s| s.as_str()).map(|s| !s.is_empty()) == Some(true)
                    })
                })
                .unwrap_or(false);
            dbg_log(&format!("grab_account_full on {url}: has_sign={has_sign}"));
            if !report.get("account_full").is_some() {
                report["account_full"] = v;
            }
        }
    }
}

const ACCOUNT_EXPR: &str = r#"
(async function(){
  function wait(pred, ms){
    return new Promise(function(r){
      var t0 = Date.now();
      (function c(){
        var v;
        try { v = pred(); } catch(e) { v = null; }
        if (v) return r(v);
        if (Date.now() - t0 > ms) return r(null);
        setTimeout(c, 200);
      })();
    });
  }
  var acc = await wait(function(){ return window.chrome && window.chrome.account && window.chrome.account.getUserInfo; }, 10000);
  var out = { href: location.href.slice(0, 50) };
  if (!acc) { out.err = 'no account api'; return JSON.stringify(out); }
  var r = await new Promise(function(res){
    var done = false;
    function f(v){ if (!done) { done = true; res(v); } }
    try {
      var p = window.chrome.account.getUserInfo({}, f);
      if (p && typeof p.then === 'function') p.then(f, function(){});
    } catch(e){}
    setTimeout(function(){ if (!done) { done = true; res(null); } }, 8000);
  });
  var d = (r && (r.data || r)) || {};
  out.sign_wg = (d.sign_wg || '').slice(0, 50);
  out.vcode = (d.vcode || '').slice(0, 30);
  out.kps = (d.kps_wg || '').slice(0, 15);
  out.uId = d.uId || '';
  if (d.sign_wg && d.vcode) out.full = r;
  return JSON.stringify(out);
})()
"#;

fn grab_cookies(report: &mut Value) {
    dbg_log("grabbing cookies via Storage.getCookies");
    let ver = match http_req(cdp_port(), "GET", "/json/version")
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
    {
        Some(v) => v,
        None => {
            dbg_log("json/version failed");
            return;
        }
    };
    let bws_url = match ver.get("webSocketDebuggerUrl").and_then(|x| x.as_str()) {
        Some(u) => u.to_string(),
        None => {
            dbg_log("no browser ws url");
            return;
        }
    };
    let mut bws = match ws_connect_url(&bws_url) {
        Ok(w) => w,
        Err(e) => {
            dbg_log(&format!("browser ws connect failed: {e}"));
            return;
        }
    };
    let resp = match cdp_call(&mut bws, "Storage.getCookies", serde_json::json!({}), 30) {
        Ok(r) => r,
        Err(e) => {
            dbg_log(&format!("Storage.getCookies failed: {e}"));
            return;
        }
    };
    let all = resp
        .pointer("/result/cookies")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let keep: Vec<Value> = all
        .into_iter()
        .filter(|c| {
            let d = c.get("domain").and_then(|x| x.as_str()).unwrap_or("");
            d.contains("qianwen.com") || d.contains("quark")
        })
        .collect();
    dbg_log(&format!("cookies kept {} of total", keep.len()));
    if !keep.is_empty() {
        report["cookies"] = Value::Array(keep);
    }
}

fn dbg_log(msg: &str) {
    use std::io::Write as _;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = std::fs::create_dir_all(config::app_dir());
    let path = config::app_dir().join("grab-debug.log");
    if let Ok(m) = std::fs::metadata(&path) {
        if m.len() > 2 * 1024 * 1024 {
            let _ = std::fs::rename(&path, config::app_dir().join("grab-debug.log.1"));
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

fn field<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
    v.get(name).filter(|x| !x.is_null())
}

fn apply_report(parsed: &Value) -> (bool, String) {
    let ucparams = field(parsed, "ucparams").cloned();
    let mut account = field(parsed, "account").cloned();
    if let Some(np) = parsed.get("net_params") {
        let sign_wg = np.get("sign_wg").and_then(|x| x.as_str()).unwrap_or("");
        let vcode = np.get("vcode").and_then(|x| x.as_str()).unwrap_or("");
        if !sign_wg.is_empty() && !vcode.is_empty() {
            let mut merged = account.clone().unwrap_or_else(|| serde_json::json!({}));
            let has_data = merged.get("data").map(|d| d.is_object()).unwrap_or(false);
            {
                let dst = if has_data {
                    merged.get_mut("data").unwrap()
                } else {
                    &mut merged
                };
                if let Some(obj) = dst.as_object_mut() {
                    let old_sign = obj
                        .get("sign_wg")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    if old_sign.is_empty() {
                        obj.insert("sign_wg".into(), Value::String(sign_wg.to_string()));
                    }
                    let old_vcode = obj.get("vcode").and_then(|x| x.as_str()).unwrap_or("");
                    if old_vcode.is_empty() {
                        obj.insert("vcode".into(), Value::String(vcode.to_string()));
                    }
                }
            }
            account = Some(merged);
        }
    }
    let public_params = field(parsed, "publicParams").cloned();
    let creds = Creds {
        grabbed_at: now_unix(),
        ua: parsed.get("ua").and_then(|x| x.as_str()).map(String::from),
        ucparams: ucparams.clone(),
        account: account.clone(),
        public_params: public_params.clone(),
        base_info: field(parsed, "baseInfo").cloned(),
        sdk: field(parsed, "sdk").cloned(),
        has_ucapi: parsed.get("hasUcapi").and_then(|x| x.as_bool()),
        cookies: parsed
            .get("cookies")
            .and_then(|x| x.as_array())
            .cloned(),
    };

    let ut = ucparams
        .as_ref()
        .or(public_params.as_ref())
        .and_then(|v| v.get("ut"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    if ut.is_empty() {
        let hint = if creds.has_ucapi == Some(false) {
            "未能从夸克浏览器取到 ut 公参（ucapi/publicParamsPrivate 均无返回），请把 %APPDATA%\\QuarkReflex\\creds-report.json 反馈给开发者。"
        } else {
            "未取到 ut 公参，请把 %APPDATA%\\QuarkReflex\\creds-report.json 反馈给开发者。"
        };
        return (false, hint.to_string());
    }
    let acc = account.as_ref().map(|a| a.get("data").unwrap_or(a));
    let kps_wg = acc
        .and_then(|a| a.get("kps_wg"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    if kps_wg.is_empty() {
        return (
            false,
            "已取到 ut，但夸克浏览器未登录或未取到登录态（kps_wg 缺失）。请先在夸克浏览器内登录夸克账号，然后重新获取。"
                .to_string(),
        );
    }

    match serde_json::to_string_pretty(&creds) {
        Ok(json) => {
            if std::fs::write(cred_path(), json).is_err() {
                return (false, "凭证保存失败（无法写入 creds.json）".to_string());
            }
        }
        Err(e) => return (false, format!("凭证序列化失败：{e}")),
    }
    (true, "凭证获取成功，聊天页即将刷新。".to_string())
}

pub fn handle_qrcred(
    key: &str,
    req: wry::http::Request<Vec<u8>>,
) -> wry::http::Response<std::borrow::Cow<'static, [u8]>> {
    let uri = req.uri().to_string();
    let (path, query) = match uri.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (uri.clone(), String::new()),
    };
    let is_poll = path.ends_with("/agentout");
    if !is_poll {
        dbg_log(&format!(
            "qrcred req: {} {} body={}",
            req.method(),
            path,
            String::from_utf8_lossy(req.body()).len()
        ));
    }
    let ok = query
        .split('&')
        .find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            if k == "k" {
                Some(v.to_string())
            } else {
                None
            }
        })
        .as_deref()
        == Some(key);
    let build = |status: u16, ctype: &str, body: Vec<u8>| {
        wry::http::Response::builder()
            .status(status)
            .header("Content-Type", ctype)
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
            .header("Access-Control-Allow-Headers", "Content-Type, *")
            .header("Cache-Control", "no-store")
            .body(std::borrow::Cow::Owned(body))
            .unwrap_or_else(|_| {
                wry::http::Response::builder()
                    .status(500)
                    .body(std::borrow::Cow::Borrowed(b"internal error".as_slice()))
                    .unwrap()
            })
    };
    if req.method() == wry::http::Method::OPTIONS {
        return build(204, "text/plain", Vec::new());
    }
    if !ok {
        return build(403, "text/plain", b"forbidden".to_vec());
    }
    if path.ends_with("/creds.json") {
        match std::fs::read(cred_path()) {
            Ok(bytes) => return build(200, "application/json", bytes),
            Err(_) => return build(200, "application/json", b"{\"none\":true}".to_vec()),
        }
    }
    if path.ends_with("/agentout") {
        let q = take_agent_queue();
        let body = serde_json::to_string(&q).unwrap_or_else(|_| "[]".into());
        return build(200, "application/json", body.into_bytes());
    }
    if path.ends_with("/call") {
        if req.method() != wry::http::Method::POST {
            return build(405, "text/plain", b"method not allowed".to_vec());
        }
        let body = req.body().clone();
        let parsed: Result<Value, _> = serde_json::from_slice(&body);
        let Ok(v) = parsed else {
            return build(400, "application/json", b"{\"err\":\"bad json\"}".to_vec());
        };
        let api = v.get("api").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let args: Vec<Value> = v
            .get("args")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let result = bridge_call(&api, &args);
        if let Err(e) = result.as_ref() {
            dbg_log(&format!("bridge /call {api} err: {e}"));
        }
        let out = match result {
            Ok(val) => serde_json::json!({ "ok": true, "v": val }),
            Err(e) => serde_json::json!({ "ok": false, "err": e }),
        };
        return build(
            200,
            "application/json",
            serde_json::to_string(&out).unwrap_or_default().into_bytes(),
        );
    }
    build(404, "text/plain", b"not found".to_vec())
}

pub fn build_inject_js(key: &str) -> String {
    BRIDGE_JS.replace("__KEY__", key)
}

const GRAB_EXPR: &str = r#"
(async function(){
  function wait(pred, ms){
    return new Promise(function(r){
      var t0 = Date.now();
      (function c(){
        var v;
        try { v = pred(); } catch(e) { v = null; }
        if (v) return r(v);
        if (Date.now() - t0 > ms) return r(null);
        setTimeout(c, 200);
      })();
    });
  }
  var out = {};
  out.ua = navigator.userAgent;
  out.hasUcapi = !!(window.ucapi && window.ucapi.invoke);
  out.hasSDK = !!window.__QIANWEN_CHAT_SDK__;
  out.chromeKeys = window.chrome ? Object.keys(window.chrome) : [];
  var UC_STR = 'dndsfrpfbibdosvessbtbmnilauputogpintnwmtsvcppcprsnnnchmicckpuaxs';
  if (out.hasUcapi) {
    out.ucparams = await new Promise(function(res){
      var done = false;
      function fin(v){ if (!done) { done = true; res(v); } }
      window.ucapi.invoke('biz.ucparams', { params: UC_STR, success: fin, fail: function(){ fin(null); } });
      setTimeout(function(){ fin(null); }, 8000);
    });
  }
  var accApi = await wait(function(){ return window.chrome && window.chrome.account && window.chrome.account.getUserInfo; }, 15000);
  if (accApi) {
    out.account = await new Promise(function(res){
      var done = false;
      function f(v){ if (!done) { done = true; res(v); } }
      try {
        var p = window.chrome.account.getUserInfo({}, f);
        if (p && typeof p.then === 'function') p.then(f, function(){});
      } catch(e) {}
      setTimeout(function(){ if (!done) { done = true; res(null); } }, 8000);
    });
  }
  try {
    if (window.chrome && window.chrome.publicParamsPrivate && window.chrome.publicParamsPrivate.ucParams) {
      out.publicParams = await window.chrome.publicParamsPrivate.ucParams(UC_STR);
    }
  } catch(e) {}
  try {
    if (window.chrome && window.chrome.quarkDebugPagePrivate && window.chrome.quarkDebugPagePrivate.getBaseInfo) {
      out.baseInfo = await window.chrome.quarkDebugPagePrivate.getBaseInfo();
    }
  } catch(e) {}
  if (window.__QIANWEN_CHAT_SDK__) {
    try {
      out.sdk = { utdid: await window.__QIANWEN_CHAT_SDK__.getUtdId({ scene: 'report' }) };
    } catch(e) {}
  }
  return JSON.stringify(out);
})()
"#;

const BRIDGE_JS: &str = r#"
(function(){
  try {
    var NL = sessionStorage.getItem('__qrNavLog') || '';
    sessionStorage.setItem('__qrNavLog', NL + '>' + location.pathname.slice(0, 30) + '@' + (Date.now() % 1000000));
  } catch(e){}
  window.__qrErrLog = [];
  try {
    window.addEventListener('error', function(e){ try{window.__qrErrLog.push(String(e.message).slice(0, 150) + ' @' + String(e.filename||'').slice(-55) + ':' + (e.lineno||0) + ':' + (e.colno||0))}catch(x){} });
    window.addEventListener('unhandledrejection', function(e){ try{var r=e&&e.reason;window.__qrErrLog.push('rej:' + String(r&&(r.stack||r.message)||r).slice(0, 300))}catch(x){} });
  } catch(e){}
  try {
    ['error', 'warn'].forEach(function(lv){
      var oc = console[lv];
      if (typeof oc === 'function') console[lv] = function(){
        try {
          var a = Array.prototype.map.call(arguments, function(x){ try { return typeof x === 'string' ? x : (x && x.message) ? x.message : JSON.stringify(x); } catch(e2){ return String(x); } }).join(' ');
          var st = '';
          for (var i = 0; i < arguments.length; i++) {
            if (arguments[i] && arguments[i].stack) { st = ' ST:' + String(arguments[i].stack).replace(/\n/g, '<').slice(0, 600); break; }
          }
          window.__qrErrLog.push('c' + lv[0] + ':' + a.slice(0, 200) + st + ' @C' + String((new Error()).stack || '').replace(/\n/g, '<').slice(0, 200));
        } catch(e2){}
        return oc.apply(console, arguments);
      };
    });
  } catch(e){}
  try {
    window.__qrChromeLog = window.__qrChromeLog || [];
    var ogi = document.getElementById.bind(document);
    document.getElementById = function(id){
      var r = ogi(id);
      try { if (id === 'ice-container') window.__qrChromeLog.push('gEBI:ice:' + (r ? 'found' : 'NULL')); } catch(e){}
      return r;
    };
  } catch(e){}
  try {
    var iceSeen = false;
    var mo = new MutationObserver(function(muts){
      try {
        for (var i = 0; i < muts.length; i++) {
          var mu = muts[i];
          if (!mu.addedNodes) continue;
          for (var j = 0; j < mu.addedNodes.length; j++) {
            var n = mu.addedNodes[j];
            if (n.nodeType !== 1) continue;
            if (n.id === 'ice-container' && !iceSeen) {
              iceSeen = true;
              window.__qrChromeLog.push('ICE:added');
              var mo2 = new MutationObserver(function(){
                try { window.__qrChromeLog.push('ICE:children:' + (ogi('ice-container') ? ogi('ice-container').children.length : '?')); } catch(e){}
              });
              mo2.observe(n, { childList: true, subtree: false });
            }
          }
        }
      } catch(e){}
    });
    mo.observe(document.documentElement, { childList: true, subtree: true });
  } catch(e){}
  if (window.__qrBridgeInstalled) return;
  window.__qrBridgeInstalled = 1;
  var onChat = location.host === 'www.qianwen.com' && location.pathname.indexOf('/quarkchat') === 0;
  window.__qrBridgeState = { onChat: onChat, xhrStatus: -1, parsed: false, ut: null };
  if (!onChat) return;
  if (!window.quantum) {
    var rmbLs = {};
    var rmbOn = function(ev, cb){
      if (typeof cb !== 'function') return function(){};
      (rmbLs[ev] = rmbLs[ev] || []).push(cb);
      return function(){ try { rmbLs[ev] = rmbLs[ev].filter(function(x){ return x !== cb; }); } catch(e){} };
    };
    var rmbOff = function(ev, cb){
      try { if (rmbLs[ev]) rmbLs[ev] = rmbLs[ev].filter(function(x){ return x !== cb; }); } catch(e){}
    };
    window.quantum = {
      rmb: {
        on: rmbOn,
        off: rmbOff,
        removeListener: rmbOff,
        requestStateChangeNotifications: function(){
          setTimeout(function(){
            (rmbLs['channel-state-changed'] || []).forEach(function(cb){ try { cb(null, 'conneceted'); } catch(e){} });
          }, 50);
        },
        registerChannel: function(){ return Promise.resolve(true); },
        unregisterChannel: function(){ return Promise.resolve(true); }
      },
      ipc: { consume: function(){ return null; } },
      wpk: {
        _jsonLog: function(){ return null; },
        _send: function(){ return null; }
      }
    };
  }
  window.__qrApiLog = [];
  try {
    var of = window.fetch;
    if (of) window.fetch = function(u, o){
      return of.apply(this, arguments).then(function(r){
        try {
          var url = String(typeof u === 'string' ? u : (u && u.url || ''));
          if (url.indexOf('chat-api') >= 0 || url.indexOf('quark') >= 0) {
            var rec = 'f:' + r.status + ':' + url.split('?')[0].slice(-60);
            r.clone().text().then(function(t){ try{window.__qrApiLog.push(rec + ' R:' + t.slice(0, 250))}catch(e){} }, function(){ try{window.__qrApiLog.push(rec)}catch(e){} });
          }
        } catch(e){}
        return r;
      }, function(e){ throw e; });
    };
    var ox = XMLHttpRequest.prototype.open, os = XMLHttpRequest.prototype.send;
    XMLHttpRequest.prototype.open = function(m, u){ this.__qrU = String(u); return ox.apply(this, arguments); };
    XMLHttpRequest.prototype.send = function(){ var x = this; this.addEventListener('load', function(){ try{ if (x.__qrU && (x.__qrU.indexOf('chat-api') >= 0 || x.__qrU.indexOf('quark') >= 0)) window.__qrApiLog.push('x:' + x.status + ':' + x.__qrU.split('?')[0].slice(-60)); }catch(e){} }); return os.apply(this, arguments); };
  } catch(e){}
  function snap(tag){
    try {
      var NL = sessionStorage.getItem('__qrNavLog') || '';
      var s = window.__qrBridgeState || {};
      var line = ' ' + tag + '[x' + s.xhrStatus + ',p' + (s.parsed ? 1 : 0) + ',ut' + (s.ut ? 1 : 0) + ',uc' + (window.__qrUcCalls || 0) + ']';
      sessionStorage.setItem('__qrNavLog', NL + line);
      if (window.__qrApiLog && window.__qrApiLog.length) {
        var AL = sessionStorage.getItem('__qrApiLog') || '';
        sessionStorage.setItem('__qrApiLog', AL + '|' + window.__qrApiLog.slice(-25).join(';'));
      }
      if (window.__qrErrLog && window.__qrErrLog.length) {
        var EL = sessionStorage.getItem('__qrErrLog') || '';
        sessionStorage.setItem('__qrErrLog', EL + '|' + window.__qrErrLog.slice(-12).join(';'));
      }
      var extra = '';
      try { if (window.__qrUcLog) extra += ' UC[' + window.__qrUcLog.join(',') + ']'; } catch(e){}
      try { if (window.__qrChromeLog) extra += ' CH[' + window.__qrChromeLog.join(',').slice(0, 1100) + ']'; } catch(e){}
      if (window.__qrChromeLog && window.__qrChromeLog.length) {
        try {
          var missLog = [];
          for (var i = 0; i < window.__qrChromeLog.length; i++) {
            var ln = window.__qrChromeLog[i];
            if (String(ln).indexOf('MISS:') >= 0 || String(ln).indexOf('UGET') === 0 || String(ln).indexOf('UCALL') === 0) missLog.push(ln);
          }
          if (missLog.length) {
            var ML = sessionStorage.getItem('__qrMissLog') || '';
            sessionStorage.setItem('__qrMissLog', ML + '|' + missLog.slice(-50).join(';'));
          }
        } catch(e2){}
      }
      if (extra) {
        var XL = sessionStorage.getItem('__qrXLog') || '';
        sessionStorage.setItem('__qrXLog', XL + '|' + tag + extra);
      }
    } catch(e){}
  }
  var CREDS = null;
  try {
    var x = new XMLHttpRequest();
    x.open('GET', 'https://qrcred.local/creds.json?k=__KEY__', false);
    x.send(null);
    window.__qrBridgeState.xhrStatus = x.status;
    if (x.status === 200) { try { CREDS = JSON.parse(x.responseText); } catch(e){ window.__qrBridgeState.parseErr = String(e); } }
  } catch(e){ window.__qrBridgeState.xhrErr = String(e).slice(0, 120); }
  snap('b');
  window.__qrCreds = CREDS;
  window.__qrBridgeState.parsed = !!CREDS;
  var UP = (CREDS && (CREDS.ucparams || CREDS.public_params)) || null;
  if (!CREDS || !UP || !UP.ut) return;
  window.__qrBridgeState.ut = String(UP.ut).slice(0, 14);
  function copy(v){ return v == null ? v : JSON.parse(JSON.stringify(v)); }
  function pickCodes(src, codes){
    var out = {};
    codes = String(codes || '');
    for (var i = 0; i + 1 < codes.length; i += 2) {
      var k = codes.substr(i, 2);
      if (src[k]) out[k] = src[k];
    }
    return out;
  }
  var UP = (CREDS.ucparams || CREDS.public_params);
  window.__qrUcCalls = 0;
  window.__qrUcLog = [];
  function qrBridge(api, args, cb){
    try {
      fetch('https://qrcred.local/call?k=__KEY__', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ api: api, args: args }) })
        .then(function(r){ return r.json(); })
        .then(function(j){ cb(j && j.ok ? j.v : undefined); })
        .catch(function(){ cb(undefined); });
    } catch(e) { setTimeout(function(){ cb(undefined); }, 0); }
  }
  function def(obj, prop, val){
    try { obj[prop] = val; return; } catch(e){}
    try { Object.defineProperty(obj, prop, { value: val, configurable: true, writable: true }); } catch(e2){}
  }
  function defLogged(obj, prop, val){
    var wrapped = {};
    for (var k in val) {
      (function(k){
        var fn = val[k];
        if (typeof fn === 'function') {
          wrapped[k] = function(){
            try { window.__qrChromeLog.push(prop + '.' + k); } catch(e){}
            return fn.apply(this, arguments);
          };
        } else { wrapped[k] = fn; }
      })(k);
    }
    var px;
    try {
      px = new Proxy(wrapped, {
        get: function(t, k){
          if (typeof k === 'string' && k !== 'then' && k !== 'toJSON' && !(k in t)) {
            try { window.__qrChromeLog.push('NSMISS[' + prop + ']:' + k); } catch(e){}
          }
          return t[k];
        }
      });
    } catch(e){ px = wrapped; }
    def(obj, prop, px);
  }
  function dualV(vf){
    return function(){
      var cbs = [];
      for (var i = 0; i < arguments.length; i++) if (typeof arguments[i] === 'function') cbs.push(arguments[i]);
      var val = vf.apply(this, arguments);
      if (cbs.length) cbs.forEach(function(cb){ setTimeout(function(){ try { cb(copy(val)); } catch(e){} }, 0); });
      return val;
    };
  }
  function dualP(vf){
    return function(){
      var cbs = [];
      for (var i = 0; i < arguments.length; i++) if (typeof arguments[i] === 'function') cbs.push(arguments[i]);
      var val = vf.apply(this, arguments);
      if (cbs.length) cbs.forEach(function(cb){ setTimeout(function(){ try { cb(copy(val)); } catch(e){} }, 0); });
      return Promise.resolve(val);
    };
  }
  function makeUniversalProxy(name){
    var p;
    var fn = function(){
      try { window.__qrChromeLog.push('UCALL[' + name + '](' + Array.prototype.slice.call(arguments).map(function(a){ return typeof a === 'object' ? (a && a.constructor && a.constructor.name) || '{}' : String(a); }).join(',').slice(0, 150) + ')'); } catch(e){}
      return Promise.resolve(undefined);
    };
    try {
      p = new Proxy(fn, {
        get: function(t, k){
          if (k === 'then' || k === 'toJSON') return undefined;
          if (typeof k === 'symbol') return t[k];
          try { window.__qrChromeLog.push('UGET[' + name + ']:' + String(k)); } catch(e){}
          return makeUniversalProxy(name + '.' + String(k));
        }
      });
    } catch(e){ p = fn; }
    return p;
  }
  function cmsCall(code){
    var cbs = [];
    for (var i = 0; i < arguments.length; i++) if (typeof arguments[i] === 'function') cbs.push(arguments[i]);
    return new Promise(function(res){
      qrBridge('cms', [code], function(v){
        cbs.forEach(function(cb){ setTimeout(function(){ try { cb(v); } catch(e){} }, 0); });
        res(v);
      });
    });
  }
  var chromeBase = window.chrome || {};
  try {
    window.chrome = new Proxy(chromeBase, {
      get: function(t, k){
        if (typeof k === 'string' && k !== 'then' && k !== 'toJSON' && !(k in t)) {
          try { window.__qrChromeLog.push('CRMISS:' + k); } catch(e){}
        }
        return t[k];
      }
    });
  } catch(e){ window.chrome = chromeBase; }
  window.__qrChromeLog = window.__qrChromeLog || [];


  if (CREDS.account && !chrome.account) {
    var rawAcc = CREDS.account;
    defLogged(window.chrome, 'account', {
      getUserInfo: function(){
        var cbs = [];
        for (var i = 0; i < arguments.length; i++) if (typeof arguments[i] === 'function') cbs.push(arguments[i]);
        cbs.forEach(function(cb){ setTimeout(function(){ try { cb(copy(rawAcc)); } catch(e){} }, 0); });
        return Promise.resolve(copy(rawAcc));
      },
      openLoginWindow: function(o, cb){ if (typeof cb === 'function') setTimeout(function(){ cb({ result: 'cancel' }); }, 300); },
      isCurrentUserRealName: dualV(function(){ return true; }),
      onAccountStateChange: {
        addListener: function(cb){ try { if (typeof cb === 'function') { window.__qrAccStateLs = window.__qrAccStateLs || []; window.__qrAccStateLs.push(cb); } } catch(e){} },
        removeListener: function(cb){ try { if (window.__qrAccStateLs) window.__qrAccStateLs = window.__qrAccStateLs.filter(function(x){ return x !== cb; }); } catch(e){} }
      },
      onBizEvent: { addListener: function(){}, removeListener: function(){} }
    });
  }
  if (CREDS.public_params && !(window.chrome && chrome.publicParamsPrivate)) {
    var rawPP = CREDS.public_params;
    defLogged(window.chrome, 'publicParamsPrivate', {
      ucParams: function(s, cb){
        var v = pickCodes(rawPP, s);
        if (typeof cb === 'function') setTimeout(function(){ try { cb(v); } catch(e){} }, 0);
        return Promise.resolve(v);
      }
    });
  }
  if (!(window.chrome && chrome.cmsPrivate)) {
    defLogged(window.chrome, 'cmsPrivate', {
      getAvailableCMSResource: function(code){ return cmsCall.apply(this, arguments); },
      getFirstAvailableCMSResource: function(code){ return cmsCall.apply(this, arguments); },
      getIntegrationResource: function(code){ return cmsCall.apply(this, arguments); },
      getFirstAvailableCMSResourceWithImgPack: dualP(function(){ return { data: {} }; }),
      getAllCMSResource: dualP(function(){ return { result: {} }; }),
      onCmsValueChanged: { addListener: function(){}, removeListener: function(){} }
    });
  }
  if (!(window.chrome && chrome.quarkDefaultSchemePrivate)) {
    defLogged(window.chrome, 'quarkDefaultSchemePrivate', {
      checkQuarkIsDefaultBrowser: dualV(function(){ return false; })
    });
  }
  if (!(window.chrome && chrome.quarkNativeUiHelperPrivate)) {
    defLogged(window.chrome, 'quarkNativeUiHelperPrivate', {
      getLastActiveTabbedBrowerState: dualP(function(){ return { isFullscreen: false, isIncognito: false }; })
    });
  }
  if (!(window.chrome && chrome.settingsPrivate)) {
    defLogged(window.chrome, 'settingsPrivate', {
      getPref: dualP(function(key){ return { key: key, type: 'DICTIONARY', value: {} }; })
    });
  }
  if (!(window.chrome && chrome.windows)) {
    defLogged(window.chrome, 'windows', {
      getCurrent: dualP(function(){ return { alwaysOnTop: false, focused: true, incognito: false, state: 'normal', type: 'normal' }; })
    });
  }
  if (!(window.chrome && chrome.quarkAiToolsPrivate)) {
    defLogged(window.chrome, 'quarkAiToolsPrivate', {
      getAiScreenshotKeybindingPrefs: dualP(function(){ return 'Ctrl+Shift+Z'; }),
      getAiScreenshotPrefs: dualV(function(){ return true; }),
      onAiScreenshotKeybindingPrefsChange: { addListener: function(){}, removeListener: function(){} },
      onAiScreenshotPrefsChange: { addListener: function(){}, removeListener: function(){} }
    });
  }
  if (!(window.chrome && chrome.userTrackPrivate)) {
    defLogged(window.chrome, 'userTrackPrivate', {
      exposureEvent: dualV(function(){ return null; }),
      clickEvent: dualV(function(){ return null; }),
      customEvent: dualV(function(){ return null; }),
      click: dualV(function(){ return null; }),
      custom: dualV(function(){ return null; }),
      originalEvent: dualV(function(){ return null; }),
      pageAppear: dualV(function(){ return null; }),
      pageDisappear: dualV(function(){ return null; }),
      updatePageProps: dualV(function(){ return null; })
    });
  }
  var accData = (CREDS.account && (CREDS.account.data || CREDS.account)) || {};
  var utdid = (CREDS.sdk && CREDS.sdk.utdid) || accData.utdId || null;
  if (utdid && !window.__QIANWEN_CHAT_SDK__) {
    window.__QIANWEN_CHAT_SDK__ = {
      getUtdId: function(){ return Promise.resolve(utdid); },
      checkLogin: function(){ return Promise.resolve(true); },
      getUserId: function(){
        return Promise.resolve(String(accData.uId || ''));
      },
      login: function(){}
    };
  }
  if (!(window.chrome && chrome.quarkWirelessSecurityGuardPrivate)) {
    defLogged(window.chrome, 'quarkWirelessSecurityGuardPrivate', {
      sign: function(text, tag, cb){
        qrBridge('sign', [text, tag], function(v){
          if (typeof cb === 'function') { try { setTimeout(function(){ cb(v === null ? undefined : v); }, 0); } catch(e){} }
        });
      }
    });
  }
  if (!(window.chrome && chrome.quarkBizPrivate)) {
    defLogged(window.chrome, 'quarkBizPrivate', {
      getTimestamp: function(cb){ if (typeof cb === 'function') setTimeout(function(){ try { cb(Date.now()); } catch(e){} }, 0); },
      getCurrentTabId: function(cb){ if (typeof cb === 'function') setTimeout(function(){ try { cb(1); } catch(e){} }, 0); },
      getQianwenStatus: function(cb){ if (typeof cb === 'function') setTimeout(function(){ try { cb({ installed: true }); } catch(e){} }, 0); },
      encryptOrDecrypt: function(o, cb){ if (typeof cb === 'function') setTimeout(function(){ try { cb(undefined); } catch(e){} }, 0); },
      showToastBubble: function(){},
      showNotification: function(){}
    });
  }
  if (!(window.chrome && chrome.quarkRoutePrivate)) {
    var routeExtra = null;
    defLogged(window.chrome, 'quarkRoutePrivate', {
      setExtraData: function(o, cb){ routeExtra = o; if (typeof cb === 'function') setTimeout(function(){ try { cb('ok'); } catch(e){} }, 0); },
      getExtraData: function(o, cb){ if (typeof cb === 'function') setTimeout(function(){ try { cb(routeExtra); } catch(e){} }, 0); },
      submit: function(){}
    });
  }
  if (!(window.chrome && chrome.quarkAiChatPrivate)) {
    var EPOCH = 'qr' + Date.now().toString(36) + Math.random().toString(36).slice(2, 10);
    var agentLs = new Set();
    window.__qrAgentDispatch = function(m){
      try {
        var d = '?';
        if (m && typeof m.method === 'string') d = 'm:' + m.method;
        else if (m && m.id !== undefined) d = 'r:' + String(m.id).slice(0, 8) + (m.error ? ' ERR:' + String(JSON.stringify(m.error)).slice(0, 150) : (m.result !== undefined ? ' res:' + Object.keys(m.result || {}).slice(0, 10).join(',') : ' nores'));
        window.__qrChromeLog.push('agentDispatch:' + d + ' L' + agentLs.size);
      } catch(e){}
      agentLs.forEach(function(cb){ try { cb(m); } catch(e){ try { window.__qrChromeLog.push('dispatchThrow:' + String(e).slice(0, 200)); } catch(x){} } });
    };
    function localAgentRespond(msg){
      try { window.__qrChromeLog.push('localAgentResp:' + ((msg && msg.method) || '?')); } catch(e){}
      var resp;
      if (msg && msg.method === 'initialize') {
        resp = { id: msg.id, result: { agent: { name: 'quark', version: 'qr' }, capabilities: { hostInit: false, requestPermission: true, resumableStream: true, thinkingStream: true }, protocolVersion: 9, runtime: { state: 'ready', runtimeEpoch: EPOCH, revision: 1 } } };
      } else {
        resp = { id: msg.id, kind: 'response', error: { code: -32601, message: 'QuarkReflex bridge unavailable' } };
      }
      agentLs.forEach(function(cb){ try { cb(resp); } catch(e){} });
    }
    defLogged(window.chrome, 'quarkAiChatPrivate', {
      sendToAgent: function(msg){
        try { window.__qrChromeLog.push('quarkAiChatPrivate.sendToAgent:' + ((msg && msg.method) || '?') + '#' + String(msg && msg.id).slice(0, 8)); } catch(e){}
        qrBridge('agentSend', [msg], function(v){
          if (v === 'ok') return;
          setTimeout(function(){ localAgentRespond(msg); }, 0);
        });
      },
      getAgentNotifyData: function(id, cb){
        if (typeof cb === 'function') setTimeout(function(){ try { cb(undefined); } catch(e){} }, 0);
      },
      onAgentMessage: {
        addListener: function(cb){ if (typeof cb === 'function') agentLs.add(cb); },
        removeListener: function(cb){ try { agentLs.delete(cb); } catch(e){} }
      },
      openUrlAndSidePanel: function(){}
    });
  }
  if (!(window.chrome && chrome.quarkFilePrivate)) {
    defLogged(window.chrome, 'quarkFilePrivate', {
      selectFile: dualP(function(){ return { files: [] }; }),
      showFileInFolder: function(){},
      getFileInfo: dualP(function(){ return null; }),
      readFile: dualP(function(){ return null; }),
      createFileMapping: dualP(function(){ return null; }),
      getMediaFileDuration: dualP(function(){ return null; })
    });
  }
  if (!(window.chrome && chrome.quarkDownloadsPrivate)) {
    defLogged(window.chrome, 'quarkDownloadsPrivate', {
      download: dualP(function(){ return null; })
    });
  }
  if (!(window.chrome && chrome.downloads)) {
    defLogged(window.chrome, 'downloads', {
      download: dualP(function(){ return null; }),
      search: dualP(function(){ return []; }),
      onChanged: { addListener: function(){}, removeListener: function(){} }
    });
  }
  if (!(window.chrome && chrome.quarkRmbPrivate)) {
    defLogged(window.chrome, 'quarkRmbPrivate', {
      onMessage: { addListener: function(){}, removeListener: function(){} },
      registerChannel: dualP(function(){ return null; }),
      unregisterChannel: dualP(function(){ return null; })
    });
  }
  if (!(window.chrome && chrome.quarkScanKingPrivate)) {
    defLogged(window.chrome, 'quarkScanKingPrivate', {
      emitScanKingBizEvent: function(){},
      onScanKingBizEvent: { addListener: function(){}, removeListener: function(){} }
    });
  }
  if (!(window.chrome && chrome.quarkSystemSettingsPrivate)) {
    defLogged(window.chrome, 'quarkSystemSettingsPrivate', {
      openMicrophoneSettings: function(){}
    });
  }
  try {
    if (!(window.chrome && chrome.quarkModeSwitchPrivate)) {
      def(window.chrome, 'quarkModeSwitchPrivate', makeUniversalProxy('quarkModeSwitchPrivate'));
    }
  } catch(e){}
  window.__qrBridgeState.installed = true;
  if (!window.__qrAgentPullTimer) {
    window.__qrAgentPullTimer = setInterval(function(){
      try {
        fetch('https://qrcred.local/agentout?k=__KEY__', { method: 'GET' })
          .then(function(r){ return r.json(); })
          .then(function(list){
            try {
              if (list && list.length) {
                window.__qrChromeLog.push('agentPull:' + list.length);
                for (var i = 0; i < list.length; i++) {
                  if (window.__qrAgentDispatch) window.__qrAgentDispatch(list[i]);
                }
              }
            } catch(e){}
          })
          .catch(function(){});
      } catch(e){}
    }, 300);
  }
  setTimeout(function(){ snap('t3'); }, 3000);
  setTimeout(function(){ snap('t8'); }, 8000);
  setTimeout(function(){ snap('t20'); }, 20000);
})();
"#;