param(
 [Parameter(Mandatory=$true)][string]$ExePath,
 [Parameter(Mandatory=$true)][string]$DataDir,
 [Parameter(Mandatory=$true)][string]$ManifestSamePath,
 [Parameter(Mandatory=$true)][string]$ManifestNextPath,
 [Parameter(Mandatory=$true)][string]$PortableZipPath,
 [Parameter(Mandatory=$true)][int]$Port
)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type @'
using System;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
public static class UpdateTestNative {
 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int width, int height, uint flags);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int x, int y, uint data, UIntPtr extra);
}
public class LocalUpdateServer : IDisposable {
 private TcpListener listener;
 private Thread worker;
 private volatile bool running = true;
 private volatile byte[] manifestBytes;
 private byte[] zipBytes;
 private string zipRoute;
 public readonly ManualResetEventSlim ZipWait = new ManualResetEventSlim(true);
 public System.Collections.Concurrent.ConcurrentQueue<string> Requests = new System.Collections.Concurrent.ConcurrentQueue<string>();
 public LocalUpdateServer(int port, string manifestFile, string zipFile) {
  manifestBytes = File.ReadAllBytes(manifestFile);
  zipBytes = File.ReadAllBytes(zipFile);
  zipRoute = "/" + Path.GetFileName(zipFile);
  listener = new TcpListener(IPAddress.Loopback, port);
  listener.Start();
  worker = new Thread(ServeLoop) { IsBackground = true };
  worker.Start();
 }
 public void SetManifest(string manifestFile) {
  manifestBytes = File.ReadAllBytes(manifestFile);
 }
 private void ServeLoop() {
  while (running) {
   TcpClient client = null;
   try {
    client = listener.AcceptTcpClient();
    using (client)
    using (var stream = client.GetStream())
    using (var reader = new StreamReader(stream, Encoding.ASCII, false, 1024, true)) {
     string requestLine = reader.ReadLine() ?? "";
     Requests.Enqueue(DateTime.Now.ToString("HH:mm:ss.fff") + " " + requestLine);
     string line;
     while (!string.IsNullOrEmpty(line = reader.ReadLine())) {}
     byte[] payload = null;
     string contentType = "application/octet-stream";
     if (requestLine.StartsWith("GET /update.json ")) {
      payload = manifestBytes;
      contentType = "application/json; charset=utf-8";
     } else if (requestLine.StartsWith("GET " + zipRoute + " ")) {
      ZipWait.Wait(12000);
      payload = zipBytes;
     }
     if (payload != null) {
      byte[] header = Encoding.ASCII.GetBytes(
       "HTTP/1.1 200 OK\r\nContent-Type: " + contentType + "\r\nContent-Length: " + payload.Length + "\r\nConnection: close\r\n\r\n");
      stream.Write(header, 0, header.Length);
      stream.Write(payload, 0, payload.Length);
      stream.Flush();
      client.Client.Shutdown(SocketShutdown.Send);
      Thread.Sleep(20);
     } else {
      byte[] header = Encoding.ASCII.GetBytes("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
      stream.Write(header, 0, header.Length);
     }
    }
   } catch (Exception ex) {
    Requests.Enqueue("ERR:" + ex.Message);
    if (!running) break;
   }
  }
 }
 public void Dispose() {
  running = false;
  ZipWait.Set();
  try { listener.Stop(); } catch {}
 }
}
'@
[UpdateTestNative]::SetThreadDpiAwarenessContext([IntPtr](-4)) | Out-Null
function Pause([int]$ms=300){$w=[Diagnostics.Stopwatch]::StartNew();while($w.ElapsedMilliseconds -lt $ms){[Windows.Forms.Application]::DoEvents();Start-Sleep -Milliseconds 10}}

function NamedControl([string]$name, $type=[Windows.Automation.ControlType]::Button) {
 [VaultTestNative]::SetForegroundWindow([IntPtr](Main).Current.NativeWindowHandle) | Out-Null; Pause 80
 $match = [Windows.Automation.AndCondition]::new(
  [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $name),
  [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, $type)
 )
 $item = (Main).FindFirst([Windows.Automation.TreeScope]::Descendants, $match)
 if ($null -eq $item) {
  $r = (Main).Current.BoundingRectangle
  [UpdateTestNative]::SetCursorPos([int]($r.Left + $r.Width / 2), [int]($r.Top + $r.Height / 2)) | Out-Null
  for ($w = 0; $w -lt 5 -and $null -eq $item; $w++) {
   [UpdateTestNative]::mouse_event(0x800, 0, 0, 4294967056, [UIntPtr]::Zero); Pause 150
   $item = (Main).FindFirst([Windows.Automation.TreeScope]::Descendants, $match)
  }
 }
 if ($null -eq $item) {
  for ($t = 0; $t -lt 35 -and $null -eq $item; $t++) {
   [Windows.Forms.SendKeys]::SendWait('{TAB}'); Pause 60
   $item = (Main).FindFirst([Windows.Automation.TreeScope]::Descendants, $match)
  }
 }
 Assert ($null -ne $item) "Control missing: $name"
 $item.SetFocus(); Pause 100
 return $item
}
function InvokeControl([string]$name, [int]$delay=350) {
 (NamedControl $name).GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
 Pause $delay
}

Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing app before GUI checks'
Assert (-not (Test-Path (Join-Path $DataDir 'vault.db'))) 'Use a fresh isolated DataDir'
$currentVersion=(Get-Content -LiteralPath $ManifestSamePath -Raw | ConvertFrom-Json).version
$nextManifest=Get-Content -LiteralPath $ManifestNextPath -Raw | ConvertFrom-Json
Assert ([Diagnostics.FileVersionInfo]::GetVersionInfo($ExePath).ProductVersion -eq $currentVersion) 'Initial executable does not match the current manifest'
Assert ([version]$nextManifest.version -gt [version]$currentVersion) 'Update manifest must describe a newer version'
Assert ([IO.Path]::GetFileName($PortableZipPath) -eq $nextManifest.portable_name) 'Test archive does not match the update manifest'
$upToDate="当前已是最新版本（v$currentVersion）。"
$updateLabel="前往更新 v$($nextManifest.version)"
$downloadRequest='GET /'+[regex]::Escape($nextManifest.portable_name)+' '
New-Item -ItemType Directory -Force $DataDir | Out-Null

$portableDir = Join-Path $DataDir 'portable-app'
New-Item -ItemType Directory -Force $portableDir | Out-Null
$portableExe = Join-Path $portableDir 'Keyisle.exe'
Copy-Item -LiteralPath $ExePath -Destination $portableExe
$ExePath = $portableExe

$prevDataDir = $env:PASSWORD_MANAGER_DATA_DIR
$prevUpdateUrl = $env:KEYISLE_UPDATE_URL
$env:PASSWORD_MANAGER_DATA_DIR = $DataDir
$env:KEYISLE_UPDATE_URL = "http://127.0.0.1:$Port/update.json"
$server = [LocalUpdateServer]::new($Port, $ManifestSamePath, $PortableZipPath)
$script:process = $null

try {
 # 1. Verify dragging on locked/create screen persists across cold restart
 StartApp
 $cache = Join-Path $DataDir 'updates'
 New-Item -ItemType Directory -Force $cache | Out-Null
 $sentinel = Join-Path $cache 'active-test.part'
 Set-Content -LiteralPath $sentinel -Value 'synthetic in-progress download'
 $second = Start-Process -FilePath $ExePath -PassThru
 Assert ($second.WaitForExit(5000) -and $second.ExitCode -eq 0) 'Second launch did not hand off to the existing app'
 Assert (Test-Path -LiteralPath $sentinel) 'Second launch removed live update cache'
 Write-Output 'PASS: second instance leaves active update cache intact.'
 $lockedHwnd = [IntPtr](Main).Current.NativeWindowHandle
 [UpdateTestNative]::SetWindowPos($lockedHwnd, [IntPtr]::Zero, 165, 135, 0, 0, 0x0001 -bor 0x0004) | Out-Null
 Pause 500
 Assert (Test-Path (Join-Path $DataDir 'window.txt')) 'Dragging locked window did not save window.txt'
 $lockedBefore = (Main).Current.BoundingRectangle
 StopApp

 StartApp
 $lockedAfter = (Main).Current.BoundingRectangle
 Assert ([Math]::Abs($lockedBefore.Left - $lockedAfter.Left) -le 3 -and [Math]::Abs($lockedBefore.Top - $lockedAfter.Top) -le 3) "Locked window cold restart lost dragged position: $($lockedBefore.Left),$($lockedBefore.Top) vs $($lockedAfter.Left),$($lockedAfter.Top)"
 Write-Output 'PASS: dragging window before unlock persists position across cold restart.'

 # 2. Create vault, add account, test Settings auto-update toggle + manual Check Update button
 SetEdit 0 'GUI-Master-Only!2026'; SetEdit 1 'GUI-Master-Only!2026'; Button '创建密码库'; Pause 1500
 Button '新增账号'; SetEdit 0 'Update Survives Account'; SetEdit 1 'update.user'; SetEdit 2 'Update-Safe!2026'; Button '保存账号'

 Button '设置'
 $autoToggle = NamedControl '启动后自动检查新版本（不上传本地数据）' ([Windows.Automation.ControlType]::CheckBox)
 $autoToggle.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Toggle(); Pause 100
 Button '保存设置'
 Assert ((Get-Content -Raw (Join-Path $DataDir 'auto-update.txt')).Trim() -eq '0') 'Disabling auto-update did not persist 0'

 # Check the manifest for the running version.
 InvokeControl '检查更新' 200
 for ($i = 0; $i -lt 40 -and -not (TextExists $upToDate); $i++) { Pause 200 }
 if (-not (TextExists $upToDate)) {
  $texts = @((Main).FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Text)) | ForEach-Object { $_.Current.Name })
  $btns = @((Main).FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Button)) | ForEach-Object { "$($_.Current.Name)[en=$($_.Current.IsEnabled),off=$($_.Current.IsOffscreen)]" })
  $reqs = ($server.Requests.ToArray()) -join '; '
  throw "Up-to-date status missing; reqs=[$reqs]; btns=[$($btns -join ' | ')]; texts=$($texts -join ' | ')"
 }

 # Serve the signed update and check from Settings.
 $server.SetManifest($ManifestNextPath)
 InvokeControl '检查更新' 600
 for ($i = 0; $i -lt 30; $i++) {
  $btn = (Main).FindFirst([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $updateLabel))
  if ($null -ne $btn) { break }
  Pause 200
 }
 Assert ($null -ne $btn) 'Manual check did not report the signed update'
 Write-Output "PASS: Settings saves auto-update preference, reports v$currentVersion as current, and verifies the signed v$($nextManifest.version) update."

 # 3. Seamless in-app portable overwrite installation + auto-relaunch without manual exit
 $oldPid = $script:process.Id
 InvokeControl $updateLabel 400
 Assert (TextExists '关于 Keyisle') 'Go-to-update did not open About page'
 $server.ZipWait.Reset()
 InvokeControl '下载并覆盖安装' 200
 for ($i = 0; $i -lt 30 -and -not ($server.Requests.ToArray() -match $downloadRequest); $i++) { Pause 100 }
 Assert ($server.Requests.ToArray() -match $downloadRequest) 'Download did not reach local test server'
 InvokeControl '取消下载' 150
 Assert (TextExists '正在取消下载…') 'Cancel did not acknowledge an unfinished request'
 Assert (-not (TextExists '已取消下载。')) 'Cancel reported completion before the worker stopped'
 $server.ZipWait.Set()
 for ($i = 0; $i -lt 40 -and -not (TextExists '已取消下载。'); $i++) { Pause 150 }
 Assert (TextExists '已取消下载。') 'Cancel completion missing; retry remains blocked'
 Assert (-not $script:process.HasExited) 'Canceled update closed the running app'
 Assert (-not (Test-Path (Join-Path $cache ([IO.Path]::ChangeExtension($nextManifest.portable_name, 'part'))))) 'Canceled download left a partial file'
 Write-Output 'PASS: cancellation waits for the stalled update request to finish and permits a clean retry.'
 $original = (Get-FileHash -LiteralPath $ExePath -Algorithm SHA256).Hash
 InvokeControl '下载并覆盖安装' 300

 Assert ($script:process.WaitForExit(15000)) 'Old portable process did not exit automatically for overwrite'
 $restarted = $null
 for ($i = 0; $i -lt 60; $i++) {
  Pause 200
  $events = [VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents')
  if ($events -ne [IntPtr]::Zero) {
   [uint32]$newPid = 0
   [VaultTestNative]::GetWindowThreadProcessId($events, [ref]$newPid) | Out-Null
   if ($newPid -ne 0 -and $newPid -ne $oldPid) {
    $restarted = Get-Process -Id $newPid
    break
   }
  }
 }
 Assert ($null -ne $restarted) 'Updated portable executable did not auto-relaunch'
 $script:process = $restarted
 Pause 500
 $installed = (Get-FileHash -LiteralPath $ExePath -Algorithm SHA256).Hash
 Assert ($installed -ne $original -and $installed -eq $nextManifest.exe_sha256) 'Executable replacement did not match the verified manifest'
 Unlock 'GUI-Master-Only!2026'
 SetEdit 0 'Update Survives Account'
 Assert (TextExists 'Update Survives Account') 'Vault account did not survive in-place portable update'
 Write-Output 'PASS: one-click verified update downloads, verifies SHA-256 + exe hash, replaces running Keyisle.exe without manual exit, relaunches, and preserves vault data.'
} finally {
 $server.Dispose()
 StopApp
 $env:PASSWORD_MANAGER_DATA_DIR = $prevDataDir
 $env:KEYISLE_UPDATE_URL = $prevUpdateUrl
}
