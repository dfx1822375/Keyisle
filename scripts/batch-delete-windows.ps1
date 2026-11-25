param(
 [Parameter(Mandatory=$true)][string]$ExePath,
 [Parameter(Mandatory=$true)][string]$DataDir,
 [switch]$CaptureScreenshots
)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class BatchTestNative {
 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
}
'@
[BatchTestNative]::SetThreadDpiAwarenessContext([IntPtr](-4)) | Out-Null

$nameProperty=[Windows.Automation.AutomationElement]::NameProperty
$controlType=[Windows.Automation.AutomationElement]::ControlTypeProperty
$master='GUI-Master-Only!2026'
function Pause([int]$ms=300) {
 $timer=[Diagnostics.Stopwatch]::StartNew()
 while($timer.ElapsedMilliseconds -lt $ms) {
  [Windows.Forms.Application]::DoEvents()
  Start-Sleep -Milliseconds 10
 }
}
function CheckBox([string]$prefix) {
 $box=@(Visible (Main) $controlType ([Windows.Automation.ControlType]::CheckBox) |
     Where-Object {$_.Current.Name.StartsWith($prefix)})[0]
 Assert ($null -ne $box) "Checkbox missing: $prefix"
 return $box
}
function Check([string]$prefix) {
 $box=CheckBox $prefix
 if ($prefix.StartsWith('选择账号 ')) {
  $scale=[BatchTestNative]::GetDpiForWindow([IntPtr](Main).Current.NativeWindowHandle)/96.0
  Assert ([Math]::Abs($box.Current.BoundingRectangle.Height-24*$scale) -le 2) 'Account checkbox focus area is taller than its intended target'
 }
 $box.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Toggle()
 Pause 190
}
function AssertRightMargin($control,[int]$margin) {
 $window=(Main).Current
 $scale=[BatchTestNative]::GetDpiForWindow([IntPtr]$window.NativeWindowHandle)/96.0
 Assert ($null -ne $control) 'Missing form control to measure'
 Assert ($window.BoundingRectangle.Right-$control.Current.BoundingRectangle.Right -ge $margin*$scale-2) 'Form control overlaps the scrollbar gutter'
}
function Backups {
 Get-ChildItem -LiteralPath (Join-Path $DataDir 'backups') -Filter 'auto-*.db' -ErrorAction SilentlyContinue |
     Sort-Object Name | ForEach-Object Name
}
function Capture([string]$name,$window=(Main)) {
 if (!$CaptureScreenshots) {return}
 $h=[IntPtr]$window.Current.NativeWindowHandle
 [VaultTestNative]::SetForegroundWindow($h) | Out-Null
 Pause 180
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $h) 'Lost test window foreground before capture'
 $r=$window.Current.BoundingRectangle
 Assert ($r.Width -gt 0 -and $r.Height -gt 0 -and $r.X -ge $area.Left -and $r.Y -ge $area.Top -and $r.Right -le $area.Right -and $r.Bottom -le $area.Bottom) 'Test window does not fit the screenshot area'
 $bitmap=[Drawing.Bitmap]::new([int]$r.Width,[int]$r.Height)
 $graphics=[Drawing.Graphics]::FromImage($bitmap)
 try {
  $graphics.CopyFromScreen([int]$r.X,[int]$r.Y,0,0,$bitmap.Size)
  $bitmap.Save((Join-Path $DataDir $name),[Drawing.Imaging.ImageFormat]::Png)
 } finally {$graphics.Dispose();$bitmap.Dispose()}
}
function CapturePopup {
 if (!$CaptureScreenshots) {return}
 $backdrop.Activate()
 [VaultTestNative]::SetForegroundWindow($backdrop.Handle) | Out-Null
 $receiver.Focus() | Out-Null
 Pause 200
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $backdrop.Handle) 'Isolated fill target did not get focus'
 foreach($key in @(0x11,0x12,0x50)){[VaultTestNative]::keybd_event($key,0,0,[UIntPtr]::Zero); Pause 45}
 foreach($key in @(0x50,0x12,0x11)){[VaultTestNative]::keybd_event($key,0,2,[UIntPtr]::Zero); Pause 45}
 Pause 500
 $byPid=[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,$script:process.Id)
 $popup=@([Windows.Automation.AutomationElement]::RootElement.FindAll([Windows.Automation.TreeScope]::Children,$byPid) |
     Where-Object {$_.Current.Name -eq 'Keyisle · 快速填充' -and !$_.Current.IsOffscreen})[0]
 Assert ($null -ne $popup) 'Quick-fill popup missing'
 $edit=@(Visible $popup $controlType ([Windows.Automation.ControlType]::Edit))[0]
 $edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue('youxiang')
 Pause 300
 Capture 'quick-fill.png' $popup
 [Windows.Forms.SendKeys]::SendWait('{ESC}')
 Pause 250
 [VaultTestNative]::SetForegroundWindow([IntPtr](Main).Current.NativeWindowHandle) | Out-Null
}

Assert (!(Test-Path -LiteralPath $DataDir)) 'Use a new isolated test folder'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing Keyisle before testing'
New-Item -ItemType Directory -Path $DataDir | Out-Null
$oldDataDir=$env:PASSWORD_MANAGER_DATA_DIR
$env:PASSWORD_MANAGER_DATA_DIR=$DataDir
[IO.File]::WriteAllText((Join-Path $DataDir 'appearance.txt'),'light')
$script:process=$null
$area=[Windows.Forms.Screen]::PrimaryScreen.WorkingArea
$backdrop=[Windows.Forms.Form]::new()
$backdrop.Text='示例登录'
$backdrop.StartPosition='Manual'
$backdrop.FormBorderStyle='None'
$backdrop.Bounds=$area
$backdrop.BackColor=[Drawing.Color]::FromArgb(236,241,244)
$receiver=[Windows.Forms.TextBox]::new()
$receiver.SetBounds(20,20,220,30)
$backdrop.Controls.Add($receiver)
$demoBackupDirectory=$null
try {
 if ($CaptureScreenshots) {
  $example=Join-Path ([IO.Path]::GetPathRoot($DataDir)) 'Keyisle Demo Backups'
  Assert (!(Test-Path -LiteralPath $example)) 'The documentation backup directory already exists'
  New-Item -ItemType Directory -Path $example | Out-Null
  $demoBackupDirectory=$example
  [IO.File]::WriteAllText((Join-Path $DataDir 'backup-directory.txt'),$demoBackupDirectory)
  $backdrop.Show()
 }
 StartApp
 SetEdit 0 $master; SetEdit 1 $master; Button '创建密码库' 1500
 foreach($item in @(
     @('企业邮箱','mail.example','工作','邮件,团队'),
     @('设备管理','device.example','工作','管理'),
     @('工作 VPN','vpn.example','工作','网络'),
     @('读书清单','reader.example','个人','阅读'))) {
  Button '新增账号'
  SetEdit 0 $item[0]; SetEdit 1 $item[1]; SetEdit 2 'Synthetic-Only!2026'
  SetEdit 3 $item[2]; SetEdit 4 $item[3]
  if ($item[0] -eq '设备管理') {Check '敏感项'}
  Button '保存账号'
 }
 Assert (@(Backups).Count -eq 0) 'Creating accounts unexpectedly wrote an automatic backup'
 for ($i=0;$i -lt 3;$i++) {
  SetEdit 0 '企业邮箱'; Button '编辑账号'
  if ($i -eq 0 -and $CaptureScreenshots) {
   AssertRightMargin (@(Edits)[0]) 32
   Capture 'account-editor.png'
  }
  $user=if($i -eq 2){'mail.example'}else{"mail.example$($i+1)"}
  SetEdit 1 $user; Button '保存账号'
 }
 $originalBackups=@(Backups)
 Assert ($originalBackups.Count -eq 3) 'Editing existing accounts should create automatic backups'
 $external=Join-Path $DataDir 'manual-export.db'
 Copy-Item -LiteralPath (Join-Path $DataDir 'vault.db') -Destination $external
 SetEdit 0 ''
 if ($CaptureScreenshots) {Pause 4200; Capture 'preview.png'}
 if ($CaptureScreenshots) {
  SetEdit 0 '设备管理'; Button '查看详情'
  Button '密码库主密码'; SetEdit 0 $master; Button '验证主密码' 1400
  Capture 'account-details.png'
  Button '返回账号列表'; SetEdit 0 ''
 }
 Button '设置'
 if ($CaptureScreenshots) {
  $colorPicker=@(Visible (Main) $nameProperty '外观配色' |
      Where-Object {$_.Current.ControlType -eq [Windows.Automation.ControlType]::ComboBox})[0]
  AssertRightMargin $colorPicker 42
  Capture 'settings.png'
 }
 Button '返回账号列表'
 CapturePopup

 Button '批量选择'; Check '选择账号 工作 VPN'; Check '选择账号 读书清单'
 Assert (TextExists '已选 2 个账号') 'Account selection count mismatch'
 SetEdit 0 '企业邮箱'
 Assert (TextExists '已选 0 个账号') 'Search must clear account selections, including hidden rows'
 SetEdit 0 ''
 Check '选择账号 工作 VPN'; Check '选择账号 敏感 · 设备管理'
 Button '删除所选账号'; Button '取消删除'
 Assert (TextExists '工作 VPN') 'Cancel removed a selected account'
 Button '删除所选账号'; Button '确认删除'
 Assert (TextExists '敏感项验证') 'Batch containing a sensitive account bypassed verification'
 Capture 'verification.png'
 Button '密码库主密码'; SetEdit 0 'Wrong-Only!2026'; Button '验证主密码' 1200
 Assert (TextExists '敏感项验证') 'Wrong master password advanced the batch'
 Button '取消验证'
 Assert ((TextExists '工作 VPN') -and (TextExists '敏感 · 设备管理')) 'Failed verification partially deleted the batch'
 Button '删除所选账号'; Button '确认删除'
 Button '密码库主密码'; SetEdit 0 $master; Button '验证主密码' 1400
 Assert (!(TextExists '工作 VPN') -and !(TextExists '敏感 · 设备管理')) 'Verified batch did not delete both selected accounts'
 Assert ((TextExists '企业邮箱') -and (TextExists '读书清单')) 'Verified batch deleted an unselected account'
 Assert (((Backups) -join '|') -eq ($originalBackups -join '|')) 'Batch account deletion created or removed a backup'
 Button '退出批量选择'
 SetEdit 0 '读书清单'; Button '删除账号'; Button '确认删除'
 Assert (!(TextExists '读书清单')) 'Single account deletion failed'
 Assert (((Backups) -join '|') -eq ($originalBackups -join '|')) 'Single account deletion created or removed a backup'
 Write-Output 'PASS: filter reset, confirmation cancellation, single verification for a mixed batch, exact account deletion and no automatic backup.'

 Button '备份管理'; Button '批量选择'
 foreach($name in @($originalBackups[0],$originalBackups[2])) {
  $box=@(Visible (Main) $controlType ([Windows.Automation.ControlType]::CheckBox) |
      Where-Object {$_.Current.Name.StartsWith('选择备份 ') -and $_.Current.Name.EndsWith($name)})[0]
  Assert ($null -ne $box) "Backup checkbox missing: $name"
  $box.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Toggle()
  Pause 190
 }
 Assert (TextExists '已选 2 份备份') 'Backup selection count mismatch'
 $backupBox=CheckBox '选择备份'
 $scale=[BatchTestNative]::GetDpiForWindow([IntPtr](Main).Current.NativeWindowHandle)/96.0
 Assert ([Math]::Abs($backupBox.Current.BoundingRectangle.Height-24*$scale) -le 2) 'Backup checkbox focus area is taller than its intended target'
 Capture 'backup-manager.png'
 Button '删除选中备份'; Button '取消备份操作'
 Assert (@(Backups).Count -eq 3) 'Cancel removed backups'
 for ($attempt=0;$attempt -lt 5;$attempt++) {
  Button '删除选中备份'; SetEdit 0 'Wrong-Only!2026'; Button '确认删除所选备份' 1200
  Assert (@(Backups).Count -eq 3) 'Wrong master password removed backups'
 }
 Assert (@(Visible (Main) $controlType ([Windows.Automation.ControlType]::Text) |
     Where-Object {$_.Current.Name -like '*多次验证失败*'}).Count -gt 0) 'Backup deletion did not start shared cooldown'
 Button '删除选中备份'; SetEdit 0 $master; Button '确认删除所选备份'
 Assert (@(Backups).Count -eq 3) 'Cooldown allowed immediate backup deletion'
 Button '取消备份操作'; Button '返回账号列表'; Button '锁定'
 SetEdit 0 $master; Button '解锁'
 Assert (TextExists '解锁密码库') 'Locking escaped the backup deletion cooldown'
 Pause 5100
 Unlock $master
 Button '备份管理'; Button '批量选择'
 foreach($name in @($originalBackups[0],$originalBackups[2])) {
  $box=@(Visible (Main) $controlType ([Windows.Automation.ControlType]::CheckBox) |
      Where-Object {$_.Current.Name.EndsWith($name)})[0]
  Assert ($null -ne $box) "Backup checkbox missing after unlock: $name"
  $box.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Toggle(); Pause 190
 }
 Button '删除选中备份'; SetEdit 0 $master; Button '确认删除所选备份' 1400
 Assert (((Backups) -join '|') -eq $originalBackups[1]) 'Batch backup deletion did not match the exact selection'
 Assert ((Test-Path -LiteralPath $external) -and (Test-Path -LiteralPath (Join-Path $DataDir 'vault.db'))) 'Batch modified a vault or external export'
 Button '返回账号列表'; SetEdit 0 ''
 Assert (TextExists '企业邮箱') 'Active vault state changed during backup deletion'
 Write-Output 'PASS: backup cancellation, master verification and deletion of only the checked managed files.'
} finally {
 if ($script:process -and !$script:process.HasExited) {StopApp}
 $backdrop.Dispose()
 $env:PASSWORD_MANAGER_DATA_DIR=$oldDataDir
 if ($demoBackupDirectory) {[IO.Directory]::Delete($demoBackupDirectory)}
}
