param([Parameter(Mandatory=$true)][string]$ExePath, [Parameter(Mandatory=$true)][string]$DataDir)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type @'
using System;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;
public sealed class SlowUpdateTest : IDisposable {
 private readonly TcpListener listener = new TcpListener(IPAddress.Loopback, 0);
 private readonly Thread worker;
 public int Port { get { return ((IPEndPoint)listener.LocalEndpoint).Port; } }
 public SlowUpdateTest() {
  listener.Start();
  worker = new Thread(Serve) { IsBackground = true };
  worker.Start();
 }
 private void Serve() {
  try { using (var client = listener.AcceptTcpClient()) {
   var reader = new System.IO.StreamReader(client.GetStream(), Encoding.ASCII, false, 1024, true);
   string line;
   while (!String.IsNullOrEmpty(line = reader.ReadLine())) {}
   Thread.Sleep(4500);
   var payload = Encoding.ASCII.GetBytes("HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
   client.GetStream().Write(payload, 0, payload.Length);
  }} catch (SocketException) {} catch (System.IO.IOException) {}
 }
 public void Dispose() { listener.Stop(); }
}
'@
$master='GUI-Master-Only!2026'
$secret='Visible-Synthetic-Only!2026'
Assert (!(Test-Path -LiteralPath $DataDir)) 'Use a new isolated test folder'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing Keyisle before testing'
New-Item -ItemType Directory -Path $DataDir | Out-Null
[IO.File]::WriteAllText((Join-Path $DataDir 'auto-update.txt'),'0')
$server=[SlowUpdateTest]::new()
$env:PASSWORD_MANAGER_DATA_DIR=$DataDir
$env:KEYISLE_UPDATE_URL="http://127.0.0.1:$($server.Port)/update.json"
$nameProperty=[Windows.Automation.AutomationElement]::NameProperty
function Named([string]$name) {
 $item=@(Visible (Main) $nameProperty $name)[0]
 Assert ($null -ne $item) "Control missing: $name"
 return $item
}
function Press([string]$keys,$control) {
 if ($control) {$control.SetFocus()}
 $handle=[IntPtr](Main).Current.NativeWindowHandle
 [VaultTestNative]::SetForegroundWindow($handle) | Out-Null
 Pause 100
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $handle) 'Test window lost keyboard focus'
 [Windows.Forms.SendKeys]::SendWait($keys)
 Pause 180
}
function BackupDirectory {return @(Edits | Where-Object {$_.Current.Name -eq '默认备份目录'})[0]}
function Value($item) {return $item.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value}
function HasText([string]$fragment) {
 return @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text) |
     Where-Object {$_.Current.Name.Contains($fragment)}).Count -gt 0
}
try {
 StartApp -NoFocus
 $confirmTop=@(Edits)[1].Current.BoundingRectangle.Top
 SetEdit 0 '1234567890123456'
 Assert (HasText '容易猜到') 'Create did not warn about a simple master'
 Assert (@(Edits)[1].Current.BoundingRectangle.Top -eq $confirmTop) 'Weak hint moved the confirmation field'
 SetEdit 0 $master
 Assert ((HasText '主密码强度良好') -and @(Edits)[1].Current.BoundingRectangle.Top -eq $confirmTop) 'Strong hint is missing or changed the layout'
 Button '创建密码库'
 SetEdit 0 '1234567890123456'
 Assert ((HasText '容易猜到') -and (HasText '两次主密码不一致')) 'Create did not show both warnings'
 $createBounds=(Main).Current.BoundingRectangle
 foreach ($action in @('创建密码库','恢复备份')) {
  $actionBounds=(Named $action).Current.BoundingRectangle
  Assert ($actionBounds.Height -gt 0 -and $actionBounds.Bottom -le $createBounds.Bottom) "Create action clipped: $action"
 }
 SetEdit 0 $master; SetEdit 1 $master; Button '创建密码库' 1200
 Assert (TextExists '暂无账号') 'Empty account list did not show its empty state'
 Button '新增账号'
 SetEdit 0 'Keyisle Fixture'; SetEdit 1 'fixture-user'; SetEdit 2 $secret; SetEdit 3 '工作'; SetEdit 4 'VPN,团队'; Button '保存账号'
 Button '查看详情'
 Assert ((TextExists '登录凭据') -and (TextExists '分类与备注')) 'Details grouping missing'
 Assert ((Named '详情密码').Current.IsPassword) 'Details was unmasked by default'
 Assert ((Value (Named '详情密码')) -eq '') 'Masked details field contains password data'
 (Named '显示详情密码').GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();Pause
 Assert ((Value (Named '详情密码')) -eq $secret) 'Explicit reveal did not load the right password'
 (Named '隐藏详情密码').GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();Pause
 Assert ((Named '详情密码').Current.IsPassword -and (Value (Named '详情密码')) -eq '') 'Hide retained the plaintext field value'
 Button '返回账号列表'
 Write-Output 'PASS: displayed password loads on demand and the masked field remains empty.'
 Button '新增账号'
 SetEdit 0 'Generator Fixture'
 Press '{UP}' (Named '生成密码长度')
 (Named '包含符号').GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Toggle();Pause
 Button '生成密码'
 Assert (HasText '已生成 16 位随机密码') 'Generator feedback does not match the selected length'
 Press '{HOME}{DOWN}' (Named '选择已有分类')
 Assert ((Value (@(Edits)[3])) -eq '工作') 'Existing category did not fill editor'
 Press '{HOME}{DOWN}' (Named '添加已有标签')
 Assert ((Value (@(Edits)[4])) -eq '') 'Browsing added a tag without confirmation'
 Button '添加标签'
 Assert (-not (Named '添加标签').Current.IsEnabled) 'Tag picker did not reset after adding'
 Press '{HOME}{DOWN}' (Named '添加已有标签')
 Button '添加标签'
 Assert ((Value (@(Edits)[4])) -eq 'VPN') 'Tag was not added exactly once'
 Button '保存账号';Button '查看详情'
 (Named '显示详情密码').GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();Pause
 $generated=Value (Named '详情密码')
 Assert ($generated.Length -eq 16 -and $generated -cmatch '^[A-Za-z0-9]{16}$') 'Generator did not honor 16 alphanumeric characters'
 Button '返回账号列表'
 Button '新增账号'
 Assert ((Value (@(Edits)[3])) -eq '' -and -not (Named '添加标签').Current.IsEnabled) 'New editor retained previous selections'
 Press '{HOME}{DOWN}' (Named '选择已有分类')
 Assert ((Value (@(Edits)[3])) -eq '工作') 'The same category could not be selected in a new editor'
 Button '取消编辑';Button '放弃修改'
 SetEdit 0 'No-Matching-Fixture'
 Assert (TextExists '没有匹配的账号') 'No-match empty state missing'
 Button '清除筛选'
 Assert (TextExists '2 个账号') 'Empty-state action did not reset search'
 Write-Output 'PASS: generator honors length and symbols, category can be chosen, and tag confirmation prevents duplicates.'
 Button '设置';Button '快捷填充';Button '录制快捷键'
 Assert (TextExists '取消录制') 'Recorder did not start'
 Press '{ESC}' $null
 Assert ((TextExists '设置') -and (TextExists '录制快捷键')) 'First Esc should cancel recording and keep Settings open'
 Button '录制快捷键';Button '备份与维护';Button '主密码与本地备份'
 Button '返回设置';Button '快捷填充'
 Assert (TextExists '录制快捷键') 'Recorder stayed active after maintenance navigation'
 Button '录制快捷键';Button '备份与维护';Button '本地备份列表'
 Button '返回设置';Button '快捷填充'
 Assert (TextExists '录制快捷键') 'Recorder stayed active after backup page navigation'
 Button '备份与维护'
 $pathEdit=BackupDirectory
 $originalPath=Value $pathEdit
 $pathEdit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($DataDir)
 Button '返回账号列表'
 Assert (TextExists '保留当前修改？') 'Dirty Settings did not confirm leaving'
 Button '继续编辑';Button '备份与维护'
 Assert ((Value (BackupDirectory)) -eq $DataDir) 'Continuing lost the settings draft'
 Button '启动与更新';Button '关于 Keyisle';Button '关闭窗口'
 Assert (TextExists '保留当前修改？') 'Closing About lost the settings draft'
 Button '继续编辑';Button '备份与维护'
 Assert ((Value (BackupDirectory)) -eq $DataDir) 'Continuing from About did not restore Settings and its draft'
 Button '返回账号列表';Button '放弃修改'
 Button '设置';Button '备份与维护'
 Assert ((Value (BackupDirectory)) -eq $originalPath) 'Discarded settings were unexpectedly saved'
 Write-Output 'PASS: Esc cancels only recording; leaving Settings subpages stops recording; keep/discard works.'
 Button '启动与更新'
 (Named '启动后自动检查新版本（不上传本地数据）').GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Toggle()
 Button '保存设置' 150
 Button '关于 Keyisle' 250
 Assert (HasText '正在检查版本更新') 'About did not join the background check'
 for($attempt=0;$attempt -lt 40 -and !(HasText 'HTTP 503');$attempt++) {Pause 200}
 Assert (HasText 'HTTP 503') 'Background failure was hidden in About'
 Button '返回';Button '返回账号列表'
 Write-Output 'PASS: an already-running silent update check reports failures on About without a second request.'
 Button '锁定'
 for($attempt=0;$attempt -lt 5;$attempt++) {
  SetEdit 0 'Wrong-Master-Only!'
  Button '解锁' 350
  for ($wait=0;$wait -lt 20 -and !(HasText '主密码不正确');$wait++){Pause 150}
  Assert (HasText '主密码不正确') 'Wrong master did not reach the authentication result'
 }
 Assert (HasText '多次验证失败') 'Fifth rejected master did not begin cooldown'
 SetEdit 0 $master;Button '解锁' 300
 Assert (TextExists '解锁密码库') 'Cooldown permitted an immediate retry'
 Pause 5100
 Unlock $master
 Assert (TextExists '2 个账号') 'Cooldown did not allow retry after expiration'
 Write-Output 'PASS: five incorrect attempts start a temporary cooldown and correct authentication works after expiration.'
}finally{
 $server.Dispose()
 if ($script:process) {
  StopApp
  if (!$script:process.HasExited -and $script:process.Path -eq $ExePath) {Stop-Process -Id $script:process.Id -Force}
 }
}
