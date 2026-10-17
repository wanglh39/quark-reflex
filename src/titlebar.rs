pub const HTML: &str = r##"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  html, body { height: 40px; overflow: hidden; }
  body {
    background: #232328;
    display: flex; align-items: center;
    font-family: "Segoe UI", "Microsoft YaHei", sans-serif;
    user-select: none; -webkit-user-select: none;
    cursor: default;
  }
  .brand { display: flex; align-items: center; gap: 7px; padding-left: 12px; flex: 1; height: 100%; min-width: 0; }
  .dot { width: 14px; height: 14px; border-radius: 4px; background: #3d5afe; flex: none;
         box-shadow: inset -3px -3px 0 rgba(255,255,255,0.25); }
  .name { font-size: 12px; color: #a8a8b0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .btns { display: flex; height: 100%; flex: none; }
  .btn {
    width: 40px; height: 100%;
    display: flex; align-items: center; justify-content: center;
    color: #9a9aa2; cursor: pointer; transition: background .12s, color .12s;
  }
  .btn:hover { background: rgba(255,255,255,0.10); color: #fff; }
  .btn:active { background: rgba(255,255,255,0.18); }
  .btn.on { color: #4c6fff; }
  .btn.on:hover { color: #6b85ff; }
  .btn.close:hover { background: #e81123; color: #fff; }
  svg { width: 16px; height: 16px; }
</style>
</head>
<body>
  <div class="brand" id="drag">
    <div class="dot"></div>
    <div class="name">夸克助手</div>
  </div>
  <div class="btns">
    <div class="btn" id="b-pin" title="置顶">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V6h1a2 2 0 0 0 0-4H8a2 2 0 0 0 0 4h1z"/>
      </svg>
    </div>
    <div class="btn" id="b-size" title="切换 小窗 / 阅读模式">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M5 11V5h6"/><path d="M19 13v6h-6"/><path d="M5 5l6 6"/><path d="M19 19l-6-6"/>
      </svg>
    </div>
    <div class="btn" id="b-set" title="设置">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <line x1="4" y1="7" x2="20" y2="7"/><circle cx="9" cy="7" r="2.4" fill="#232328"/>
        <line x1="4" y1="12" x2="20" y2="12"/><circle cx="15" cy="12" r="2.4" fill="#232328"/>
        <line x1="4" y1="17" x2="20" y2="17"/><circle cx="7" cy="17" r="2.4" fill="#232328"/>
      </svg>
    </div>
    <div class="btn close" id="b-col" title="收起">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <line x1="6" y1="12" x2="18" y2="12"/>
      </svg>
    </div>
  </div>
<script>
  function send(o) { try { window.ipc.postMessage(JSON.stringify(o)); } catch (e) {} }
  document.addEventListener('mousedown', function (e) {
    if (e.button !== 0) return;
    if (e.target.closest('.btn')) return;
    send({ cmd: 'drag' });
  });
  document.addEventListener('dblclick', function (e) {
    if (e.target.closest('.btn')) return;
    send({ cmd: 'toggle_size' });
  });
  document.getElementById('b-pin').onclick = function () { send({ cmd: 'pin' }); };
  document.getElementById('b-size').onclick = function () { send({ cmd: 'toggle_size' }); };
  document.getElementById('b-set').onclick = function () { send({ cmd: 'settings' }); };
  document.getElementById('b-col').onclick = function () { send({ cmd: 'collapse' }); };
  window.__qrSetPin = function (v) {
    document.getElementById('b-pin').classList.toggle('on', !!v);
  };
</script>
</body>
</html>
"##;