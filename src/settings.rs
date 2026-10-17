use crate::config::Config;

const TEMPLATE: &str = r##"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  html, body { height: 100%; overflow: hidden; }
  body {
    background: #ffffff; color: #2a313f;
    font-family: "Microsoft YaHei UI", "Microsoft YaHei", "Segoe UI", sans-serif;
    user-select: none; -webkit-user-select: none;
    display: flex; flex-direction: column;
  }
  .bar { flex: none; padding: 22px 24px 0; position: relative; }
  .bar h1 { font-size: 21px; font-weight: 700; }
  .bar .sub { font-size: 12px; color: #677082; margin-top: 6px; }
  .bar .x {
    position: absolute; top: 22px; right: 24px; width: 32px; height: 28px;
    display: flex; align-items: center; justify-content: center;
    color: #677082; cursor: pointer; border-radius: 7px;
    transition: background .12s, color .12s;
  }
  .bar .x:hover { background: #f1f4f8; color: #2a313f; }
  .main { flex: 1; overflow-y: auto; padding: 0 24px; }
  .sec { font-size: 13px; font-weight: 700; margin: 20px 0 8px; }
  .card { background: #f7f9fc; border-radius: 10px; padding: 6px 16px; }
  .row { display: flex; align-items: center; height: 56px; gap: 16px; }
  .row .lab { flex: none; width: 118px; font-size: 13px; }
  .hk {
    flex: 1; height: 40px; border-radius: 7px; background: #ffffff;
    display: flex; align-items: center; justify-content: center;
    font-size: 13px; color: #2a313f; cursor: pointer;
    border: 1px solid transparent;
    transition: border-color .12s, color .12s;
  }
  .hk:hover { border-color: #e1e7f0; }
  .hk.rec { border-color: #4366ee; color: #4366ee; }
  .hint { font-size: 12px; color: #677082; padding: 8px 2px 0; }
  .radios { display: flex; gap: 10px; }
  .radio {
    flex: 1; height: 40px; border-radius: 7px; background: #f7f9fc;
    border: 1px solid #e1e7f0; display: flex; align-items: center;
    justify-content: center; font-size: 13px; color: #2a313f; cursor: pointer;
    transition: background .12s, border-color .12s, color .12s;
  }
  .radio:hover { background: #f1f4f8; }
  .radio.sel { background: #edf2ff; border-color: #4366ee; color: #4366ee; }
  .grid { flex: 1; display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
  .num {
    display: flex; align-items: center; gap: 8px; height: 40px;
    border-radius: 7px; background: #ffffff; border: 1px solid #e1e7f0;
    padding: 0 12px; font-size: 13px;
  }
  .num input {
    width: 100%; background: none; border: none; outline: none;
    color: #2a313f; font-size: 13px; text-align: right; font-family: inherit;
  }
  select {
    width: 100%; height: 40px; border-radius: 7px; background: #f7f9fc;
    border: 1px solid #e1e7f0; color: #2a313f; font-size: 13px;
    padding: 0 10px; outline: none; font-family: inherit; cursor: pointer;
  }
  option { background: #ffffff; }
  .foot {
    flex: none; padding: 14px 24px 18px; display: flex; align-items: center; gap: 10px;
  }
  .msg { flex: 1; font-size: 12px; color: #677082; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .msg.err { color: #b13030; }
  .btn {
    flex: none; height: 34px; padding: 0 22px; border: none; border-radius: 7px;
    font-size: 13px; cursor: pointer; font-family: inherit;
    transition: background .12s;
  }
  .cancel { background: #ffffff; color: #2a313f; border: 1px solid #e1e7f0; }
  .cancel:hover { background: #edf2ff; }
  .save { background: #4366ee; color: #ffffff; }
  .save:hover { background: #3352d6; }
  .save:active { background: #2a46be; }
</style>
</head>
<body>
  <div class="bar">
    <h1>小窗设置</h1>
    <div class="sub">顺手唤起，随时问答</div>
    <div class="x" id="x">
      <svg width="12" height="12" viewBox="0 0 12 12"><path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>
    </div>
  </div>
  <div class="main">
    <div class="sec">快捷键</div>
    <div class="card">
      <div class="row">
        <div class="lab">唤出 / 收起</div>
        <div class="hk" id="hk" tabindex="0">__HOTKEY__</div>
      </div>
    </div>
    <div class="hint">点击按键框开始录入，Esc 取消，保存后生效。</div>
    <div class="sec">快捷键收起方式</div>
    <div class="radios">
      <div class="radio" data-v="minimize">最小化到任务栏</div>
      <div class="radio" data-v="hide">收进托盘</div>
    </div>
    <div class="sec">窗口尺寸</div>
    <div class="card">
      <div class="row">
        <div class="lab">小窗模式</div>
        <div class="grid">
          <div class="num">宽 <input id="cw" type="number" value="__CW__"></div>
          <div class="num">高 <input id="ch" type="number" value="__CH__"></div>
        </div>
      </div>
      <div class="row">
        <div class="lab">阅读模式</div>
        <div class="grid">
          <div class="num">宽 <input id="rw" type="number" value="__RW__"></div>
          <div class="num">高 <input id="rh" type="number" value="__RH__"></div>
        </div>
      </div>
    </div>
    <div class="sec">页面缩放</div>
    <select id="zoom">
      <option value="0.75">75%</option>
      <option value="0.8">80%</option>
      <option value="0.85">85%</option>
      <option value="0.9">90%</option>
      <option value="1.0">100%</option>
      <option value="1.1">110%</option>
      <option value="1.25">125%</option>
    </select>
  </div>
  <div class="foot">
    <div class="msg" id="msg"></div>
    <button class="btn cancel" id="cancel">取消</button>
    <button class="btn save" id="save">保存</button>
  </div>
<script>
  var pendingHotkey = '__HOTKEY__';
  function send(o) { try { window.ipc.postMessage(JSON.stringify(o)); } catch (e) {} }
  function codeLabel(c) {
    var m = { ControlLeft:'Ctrl', ControlRight:'Ctrl', ShiftLeft:'Shift', ShiftRight:'Shift',
              AltLeft:'Alt', AltRight:'Alt', MetaLeft:'Win', MetaRight:'Win',
              Space:'Space', Tab:'Tab', CapsLock:'CapsLock', Enter:'Enter', Backspace:'Backspace',
              Minus:'-', Equal:'=', Comma:',', Period:'.', Slash:'/', Semicolon:';', Quote:"'",
              BracketLeft:'[', BracketRight:']', Backslash:'\\', Backquote:'`',
              ArrowUp:'Up', ArrowDown:'Down', ArrowLeft:'Left', ArrowRight:'Right' };
    if (m[c]) return m[c];
    if (c.indexOf('Key') === 0) return c.slice(3);
    if (c.indexOf('Digit') === 0) return c.slice(5);
    if (c.indexOf('F') === 0 && c.length <= 3) return c;
    return c;
  }
  function hotkeyLabel(hk) {
    var parts = hk.split('+');
    var mods = parts.slice(0, -1).map(function (p) {
      return p.charAt(0).toUpperCase() + p.slice(1);
    });
    return mods.concat([codeLabel(parts[parts.length - 1])]).join(' + ');
  }
  var hkEl = document.getElementById('hk');
  hkEl.textContent = hotkeyLabel(pendingHotkey);
  var recording = false;
  hkEl.addEventListener('click', function () {
    recording = true;
    hkEl.classList.add('rec');
    hkEl.textContent = '请按下组合键…';
  });
  document.addEventListener('keydown', function (e) {
    if (!recording) return;
    e.preventDefault(); e.stopPropagation();
    if (e.key === 'Escape') {
      recording = false; hkEl.classList.remove('rec');
      hkEl.textContent = hotkeyLabel(pendingHotkey);
      return;
    }
    if (['Control', 'Shift', 'Alt', 'Meta'].indexOf(e.key) >= 0) return;
    var mods = [];
    if (e.ctrlKey) mods.push('ctrl');
    if (e.shiftKey) mods.push('shift');
    if (e.altKey) mods.push('alt');
    if (e.metaKey) mods.push('meta');
    if (mods.length === 0 && e.code.indexOf('F') !== 0) {
      hkEl.textContent = '需至少一个修饰键，重试…';
      return;
    }
    pendingHotkey = mods.join('+') + (mods.length ? '+' : '') + e.code;
    recording = false; hkEl.classList.remove('rec');
    hkEl.textContent = hotkeyLabel(pendingHotkey);
  });
  var collapse = '__COLLAPSE__';
  var radios = document.querySelectorAll('.radio');
  radios.forEach(function (r) {
    if (r.dataset.v === collapse) r.classList.add('sel');
    r.addEventListener('click', function () {
      collapse = r.dataset.v;
      radios.forEach(function (x) { x.classList.remove('sel'); });
      r.classList.add('sel');
    });
  });
  document.getElementById('zoom').value = '__ZOOM__';
  document.querySelector('.bar').addEventListener('mousedown', function (e) {
    if (e.target.closest('.x')) return;
    send({ cmd: 'drag' });
  });
  document.getElementById('x').onclick = function () { send({ cmd: 'close' }); };
  document.getElementById('cancel').onclick = function () { send({ cmd: 'close' }); };
  document.getElementById('save').onclick = function () {
    send({
      cmd: 'save',
      hotkey: pendingHotkey,
      collapse: collapse,
      cw: parseFloat(document.getElementById('cw').value) || 0,
      ch: parseFloat(document.getElementById('ch').value) || 0,
      rw: parseFloat(document.getElementById('rw').value) || 0,
      rh: parseFloat(document.getElementById('rh').value) || 0,
      zoom: parseFloat(document.getElementById('zoom').value) || 0.9
    });
  };
  window.__qrResult = function (msg) {
    var el = document.getElementById('msg');
    el.textContent = msg;
    el.className = 'msg err';
  };
</script>
</body>
</html>
"##;

pub fn render(config: &Config) -> String {
    TEMPLATE
        .replace("__HOTKEY__", &config.hotkey)
        .replace("__COLLAPSE__", &config.collapse_mode)
        .replace("__CW__", &format!("{}", config.compact.w))
        .replace("__CH__", &format!("{}", config.compact.h))
        .replace("__RW__", &format!("{}", config.reading.w))
        .replace("__RH__", &format!("{}", config.reading.h))
        .replace("__ZOOM__", &format!("{}", config.zoom))
}