use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const APP_DIR_NAME: &str = "QuarkReflex";
pub const CHAT_URL: &str =
    "https://www.qianwen.com/quarkchat?entry=homepage&entry_l2=bar_switch_active";
pub const BAR_H: f64 = 40.0;
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36 QuarkPC/7.3.5.1009";

pub const LOGIN_FLOW_JS: &str = r#"
(function(){
  if (window.__qrLoginFlow) return;
  window.__qrLoginFlow = 1;

  var CHAT = 'https://www.qianwen.com/quarkchat?entry=homepage&entry_l2=bar_switch_active';
  var HOME = 'https://www.qianwen.com/';
  function loggedIn(){
    if (window.__qrCreds && window.__qrCreds.account) return true;
    return /(?:^|;\s*)tongyi_sso_ticket_hash/.test(document.cookie);
  }
  function chatRendered(){
    var c = document.querySelector('#ice-container') || document.querySelector('#root');
    return !!(c && c.children.length > 0 && (document.body.innerText || '').length > 20);
  }
  function showLoginOverlay(){
    if (document.getElementById('__qrLoginOverlay')) return;
    var d = document.createElement('div');
    d.id = '__qrLoginOverlay';
    d.style.cssText = 'position:fixed;inset:0;z-index:2147483647;display:flex;align-items:center;justify-content:center;background:#fff;font-family:"Microsoft YaHei","Segoe UI",sans-serif;';
    var hint = '';
    if (!window.__qrCreds){
      hint = '<div style="font-size:12px;color:#aaa;margin-top:14px;line-height:1.6;">若已登录仍无法使用，请右键托盘图标<br>选择「获取登录凭证」</div>';
    }
    d.innerHTML = '<div style="text-align:center;padding:24px;">'
      + '<div style="font-size:17px;font-weight:600;color:#1a1a1a;margin-bottom:8px;">使用夸克助手前请先登录</div>'
      + '<div style="font-size:13px;color:#888;margin-bottom:20px;">登录后即可开始对话</div>'
      + '<button id="__qrLoginBtn" style="height:40px;padding:0 28px;border:none;border-radius:8px;background:#4c6fff;color:#fff;font-size:14px;cursor:pointer;font-family:inherit;">去登录</button>'
      + hint
      + '</div>';
    document.body.appendChild(d);
    document.getElementById('__qrLoginBtn').onclick = function(){
      try { sessionStorage.setItem('__qrLoginTried', '1'); } catch(e){}
      location.href = HOME;
    };
  }
  function hideLoginOverlay(){
    var d = document.getElementById('__qrLoginOverlay');
    if (d) d.remove();
  }
  var path = location.pathname;
  if (path.indexOf('/quarkchat') === 0){
    var checks = 0;
    var iv = setInterval(function(){
      checks++;
      if (chatRendered()){ clearInterval(iv); hideLoginOverlay(); return; }
      if (checks >= 20 && !loggedIn()){ showLoginOverlay(); }
      if (checks > 600) clearInterval(iv);
    }, 1000);
  } else if (path.indexOf('/quarkchat/download') >= 0){
    try { sessionStorage.setItem('__qrLoginTried', '1'); } catch(e){}
    location.href = HOME;
  } else if (location.host === 'www.qianwen.com'){
    if (sessionStorage.getItem('__qrLoginTried') !== '1') return;
    var n = 0;
    var iv2 = setInterval(function(){
      n++;
      if (loggedIn()){
        clearInterval(iv2);
        sessionStorage.removeItem('__qrLoginTried');
        location.href = CHAT;
        return;
      }
      if (n > 1800) clearInterval(iv2);
    }, 2000);
  }
})();
"#;

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct WinGeom {
    pub x: i32,
    pub y: i32,
    pub w: f64,
    pub h: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub hotkey: String,
    pub collapse_mode: String,
    pub compact: Size,
    pub reading: Size,
    pub reading_mode: bool,
    pub zoom: f64,
    pub pin: bool,
    pub window: Option<WinGeom>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "ctrl+space".into(),
            collapse_mode: "hide".into(),
            compact: Size { w: 410.0, h: 616.0 },
            reading: Size { w: 752.0, h: 720.0 },
            reading_mode: false,
            zoom: 0.9,
            pin: false,
            window: None,
        }
    }
}

pub fn app_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join(APP_DIR_NAME)
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.json")
}

pub fn webview_data_dir() -> PathBuf {
    app_dir().join("webview")
}

pub fn load() -> Config {
    match fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

pub fn save(config: &Config) {
    let dir = app_dir();
    let _ = fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_string_pretty(config) {
        let _ = fs::write(config_path(), json);
    }
}