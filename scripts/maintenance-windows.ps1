param([Parameter(Mandatory=$true)][string]$ExePath, [Parameter(Mandatory=$true)][string]$DataDir, [switch]$LayoutOnly)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class MaintenanceNative {
 [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
 [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr window,int command);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr window,IntPtr after,int x,int y,int width,int height,uint flags);
}
'@
trap { Write-Output $_.Exception.Message; Write-Output $_.ScriptStackTrace; throw }
if(-not $LayoutOnly){ Assert (-not (Test-Path (Join-Path $DataDir 'vault.db'))) 'Use an empty isolated test profile' }
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing app before testing'
$previous = $env:PASSWORD_MANAGER_DATA_DIR
$env:PASSWORD_MANAGER_DATA_DIR = $DataDir
$script:process = $null
$old = 'GUI-Master-Only!2026'
$new = 'GUI-New-Master!2026'
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
function Maximize {
 $h=[IntPtr](Main).Current.NativeWindowHandle;if(-not [MaintenanceNative]::IsZoomed($h)){Button '最大化或还原窗口'}
 Pause 500;Assert ([MaintenanceNative]::IsZoomed($h)) 'Custom maximize failed'
}
function OpenMaintenance { Button '设置'; Button '主密码与本地备份' }
function FormSet([string]$name,[string]$value) {
 $fields = @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Edit) | Where-Object { $_.Current.Name -eq $name })
 Assert ($fields.Count -eq 1) "Maintenance field missing: $name"
 $fields[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($value)
 Pause 80
}
function WaitDone {
 for($i=0;$i -lt 160;$i++) {
  $buttons = @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) | Where-Object { $_.Current.Name -eq '解锁' -and $_.Current.IsEnabled })
  if($buttons.Count -gt 0){ return }
  Pause 200
 }
 throw 'Maintenance did not complete within 32 seconds'
}
function MessageMatches([string]$pattern) {
 return @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text) | Where-Object {$_.Current.Name -match $pattern}).Count -gt 0
}
function Capture([string]$name) {
 $rect = (Main).Current.BoundingRectangle
 $left = [Math]::Max(0,[int]$rect.X); $top = [Math]::Max(0,[int]$rect.Y)
 $screen = [Windows.Forms.Screen]::PrimaryScreen.Bounds
 $width = [Math]::Min([int]$rect.Width,$screen.Width - $left)
 $height = [Math]::Min([int]$rect.Height,$screen.Height - $top)
 $bitmap = [Drawing.Bitmap]::new($width,$height)
 $graphics = [Drawing.Graphics]::FromImage($bitmap)
 try { $graphics.CopyFromScreen($left,$top,0,0,$bitmap.Size); $bitmap.Save((Join-Path $DataDir $name),[Drawing.Imaging.ImageFormat]::Png) }
 finally { $graphics.Dispose(); $bitmap.Dispose() }
}
function TestLayout {
 $handle = [IntPtr](Main).Current.NativeWindowHandle
 $scale = [MaintenanceNative]::GetDpiForWindow($handle) / 96.0
 [MaintenanceNative]::ShowWindow($handle,9) | Out-Null
 [MaintenanceNative]::SetWindowPos($handle,[IntPtr]::Zero,0,0,[int](680 * $scale),[int](700 * $scale),6) | Out-Null
 Pause 800
 Capture 'maintenance-narrow.png'
 $current = @(Edits | Where-Object {$_.Current.Name -eq '当前主密码'})[0]
 $current.SetFocus()
 for($step=0;$step -lt 20;$step++){
  [Windows.Forms.SendKeys]::SendWait('{TAB}');Pause 80
  if([Windows.Automation.AutomationElement]::FocusedElement.Current.Name -eq '清除全部本地备份'){break}
 }
 $focused = [Windows.Automation.AutomationElement]::FocusedElement
 Assert ($focused.Current.Name -eq '清除全部本地备份' -and -not $focused.Current.IsOffscreen) 'Keyboard navigation did not reach visible clear button in narrow window'
 [Windows.Forms.SendKeys]::SendWait('{ESC}')
 Pause 300
 Assert (TextExists '设置') 'Escape did not return from maintenance to settings'
 Button '主密码与本地备份'
 Assert (@(Edits | Where-Object {$_.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -ne ''}).Count -eq 0) 'Escape retained maintenance passwords'
 Button '返回设置'; Button '返回账号列表'
 Write-Output 'PASS: minimum-size form scrolls with keyboard focus and Escape returns to settings.'
}
try {
 if($LayoutOnly){ StartApp; Unlock $new; Maximize; OpenMaintenance; TestLayout; return }
 StartApp
 SetEdit 0 $old; SetEdit 1 $old; Button '创建密码库'; Pause 1500; Maximize
 Button '新增账号'; SetEdit 0 'Rotation test'; SetEdit 1 'synthetic.user'; SetEdit 2 'Synthetic-Account!2026'; Button '保存账号'
 Button '编辑账号'; SetEdit 0 'Rotation test edited'; Button '保存账号'
 $manual = Join-Path $DataDir 'manual-export.db'
 Button '加密备份'; DialogFile '保存加密备份（请选择新文件）' $manual
 Copy-Item (Join-Path $DataDir 'vault.db') (Join-Path $DataDir 'vault.pre-v04-1.db')
 $manualHash = (Get-FileHash $manual).Hash
 $before = @(Get-ChildItem (Join-Path $DataDir 'backups') -Filter 'auto-*.db').Count
 Assert ($before -eq 1) 'Expected one synthetic automatic backup'
 OpenMaintenance
 Assert (TextExists '本地备份：2 个') 'Managed backup count is wrong'
 Capture 'maintenance.png'
 $fields = @(Edits)
 Assert ($fields.Count -eq 3) 'Expected current/new/confirmation password fields'
 Assert (@($fields | Where-Object {-not $_.Current.IsPassword}).Count -eq 0) 'Master passwords must be masked'
 FormSet '当前主密码' $old; FormSet '新主密码' $new; FormSet '确认新主密码' 'mismatch'
 Button '修改主密码并更新本地备份'
 Assert (TextExists '两次新主密码不一致，请重新输入。') 'Mismatch was accepted'
 Button '显示新主密码'
 Assert (@(Edits | Where-Object {-not $_.Current.IsPassword}).Count -eq 1) 'New-password visibility toggle failed'
 Button '隐藏新主密码';Button '显示确认新主密码'
 Assert (@(Edits | Where-Object {-not $_.Current.IsPassword}).Count -eq 1) 'Confirmation visibility toggle failed'
 Button '隐藏确认新主密码'
 FormSet '确认新主密码' $new
 Button '返回设置'; Button '主密码与本地备份'
 Assert (@(Edits | Where-Object {$_.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -ne ''}).Count -eq 0) 'Leaving maintenance retained master inputs'
 Button '清除全部本地备份'
 Assert (Test-Path (Join-Path $DataDir 'vault.pre-v04-1.db')) 'Clear deleted a backup before confirmation'
 Button '取消清除'
 Assert (TextExists '本地备份：2 个') 'Cancel changed backup count'
 Write-Output 'PASS: backup count, masked fields, show/hide, confirmation validation, clear cancellation, input cleanup.'

 FormSet '当前主密码' 'wrong-current-master'; FormSet '新主密码' $new; FormSet '确认新主密码' $new
 Button '修改主密码并更新本地备份' 0; WaitDone
 Unlock $old
 Assert (TextExists '1 个账号') 'Wrong current master changed the live vault'
 OpenMaintenance
 FormSet '当前主密码' $old; FormSet '新主密码' $new; FormSet '确认新主密码' $new
 Button '修改主密码并更新本地备份' 0; WaitDone
 Assert (MessageMatches '主密码已修改，已同步更新 2 个本地备份') 'Password rotation did not report success'
 Assert ((Get-FileHash $manual).Hash -eq $manualHash) 'Manual export was rewritten'
 Unlock $old
 Assert (TextExists '解锁密码库') 'Old master still unlocks the active vault'
 Unlock $new
 Assert (TextExists '1 个账号') 'New master did not unlock the active vault'
 Button '编辑账号'
 Assert ((Edits)[1].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'synthetic.user') 'Rotation changed username'
 Assert ((Edits)[2].Current.IsPassword) 'Rotation editor password must remain masked'
 Button '取消编辑'
 StopApp; StartApp; Unlock $new; Maximize
 Assert (TextExists '1 个账号') 'New master failed after restart'
 Write-Output 'PASS: wrong current master refuses change; successful rotation rejects old master, accepts new master, preserves contents and survives restart.'

 OpenMaintenance
 FormSet '当前主密码' 'wrong-current-master'
 Button '清除全部本地备份'; Button '确认清除全部本地备份' 0; WaitDone
 Assert (Test-Path (Join-Path $DataDir 'vault.pre-v04-1.db')) 'Wrong master deleted backups'
 Unlock $new
 OpenMaintenance
 FormSet '当前主密码' $new
 Button '清除全部本地备份'; Button '确认清除全部本地备份' 0; WaitDone
 Assert (MessageMatches '已清除 2 个本地备份') 'Confirmed clear did not complete'
 Assert (@(Get-ChildItem (Join-Path $DataDir 'backups') -Filter 'auto-*.db').Count -eq 0) 'Automatic backups remain'
 Assert (-not (Test-Path (Join-Path $DataDir 'vault.pre-v04-1.db'))) 'Upgrade backup remains'
 Assert (Test-Path (Join-Path $DataDir 'vault.db')) 'Clear removed live vault'
 Assert ((Get-FileHash $manual).Hash -eq $manualHash) 'Clear removed or modified manual export'
 Unlock $new; OpenMaintenance
 Assert (TextExists '本地备份：0 个') 'Zero-backup state missing'
 Capture 'maintenance-empty.png'
 Button '返回设置'; Button '返回账号列表'
 Button '编辑账号'; SetEdit 0 'After clearing'; Button '保存账号'
 Assert (@(Get-ChildItem (Join-Path $DataDir 'backups') -Filter 'auto-*.db').Count -eq 1) 'Automatic backups did not resume after clear'
 Write-Output 'PASS: clear requires current master and confirmation; vault and exported copy survive; empty state and future automatic backup work.'

 OpenMaintenance; FormSet '当前主密码' $new; FormSet '新主密码' 'Do-Not-Retain!2026'; FormSet '确认新主密码' 'Do-Not-Retain!2026'
 SessionLock
 Assert (TextExists '解锁密码库') 'Session lock left maintenance open'
 Unlock $new; OpenMaintenance
 Assert (@(Edits | Where-Object {$_.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -ne ''}).Count -eq 0) 'Session lock retained maintenance passwords'
 Write-Output 'PASS: session lock closes maintenance and clears all master-password fields.'
 TestLayout
} finally {
 StopApp
 $env:PASSWORD_MANAGER_DATA_DIR = $previous
}
