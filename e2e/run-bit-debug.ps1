# 以调试端口启动 BIT release 版并探测前端渲染状态
Get-Process bit -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 1
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9333"
Start-Process "C:\Users\yxpil\Desktop\BIT\src-tauri\target\release\bit.exe"
Start-Sleep 10
netstat -ano | Select-String 9333
node C:\Users\yxpil\Desktop\BIT\e2e\cdp-probe.cjs
