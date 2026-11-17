param([Parameter(Mandatory=$true)][string]$ExePath,[Parameter(Mandatory=$true)][string]$DataDir,[switch]$LayoutOnly)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class DailyNative {
 [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int width,int height,uint flags);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
}
'@
trap { Write-Output $_.Exception.Message; Write-Output $_.ScriptStackTrace; try { Capture 'failure.png' } catch {}; throw }
if(-not $LayoutOnly){Assert (-not (Test-Path (Join-Path $DataDir 'vault.db'))) 'Use an empty isolated test profile'}
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing app first'
$previous = $env:PASSWORD_MANAGER_DATA_DIR
$env:PASSWORD_MANAGER_DATA_DIR = $DataDir
$script:process = $null
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
$cursor = [Windows.Forms.Cursor]::Position
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$runBefore = (Get-ItemProperty -LiteralPath $runKey -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager
function Maximize { $h=[IntPtr](Main).Current.NativeWindowHandle;if(-not [DailyNative]::IsZoomed($h)){Button '最大化或还原窗口'};Pause 500;Assert ([DailyNative]::IsZoomed($h)) 'Custom maximize failed' }
function WaitDone {
 for($i=0;$i -lt 160;$i++) {
  if(-not (TextExists '正在验证并处理选中的备份…')){Pause 200;return}
  Pause 200
 }
 throw 'Backup operation timed out'
}
function MatchText([string]$pattern) { return @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text) | Where-Object {$_.Current.Name -match $pattern}).Count -gt 0 }
function Row([string]$title) {
 $rows = @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) | Where-Object {$_.Current.Name -like "*，$title，*" -or $_.Current.Name -like "$title，*"})
 Assert ($rows.Count -eq 1) "Account row missing: $title"
 return $rows[0]
}
function RightClick($element) {
 $r = $element.Current.BoundingRectangle
 $script:lastRect = (Main).Current.BoundingRectangle
 $handle=[IntPtr](Main).Current.NativeWindowHandle
 [VaultTestNative]::SetForegroundWindow($handle) | Out-Null
 [DailyNative]::SetCursorPos([int]($r.X+$r.Width/2),[int]($r.Y+$r.Height/2)) | Out-Null
 [DailyNative]::mouse_event(8,0,0,0,[UIntPtr]::Zero); [DailyNative]::mouse_event(16,0,0,0,[UIntPtr]::Zero)
 Pause 350
 Capture 'right-click.png'
}
function Menu([string]$name) {
 $group=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '账号右键菜单')
 Assert ($group.Count -eq 1) 'Context menu missing'
 $items=@(Visible $group[0] ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) | Where-Object {$_.Current.Name -eq $name})
 Assert ($items.Count -eq 1) "Menu item missing: $name"
 $pattern=$null
 if($items[0].TryGetCurrentPattern([Windows.Automation.InvokePattern]::Pattern,[ref]$pattern)){$pattern.Invoke()}
 else {
  $rect=$items[0].Current.BoundingRectangle
  [DailyNative]::SetCursorPos([int]($rect.X+$rect.Width/2),[int]($rect.Y+$rect.Height/2))|Out-Null
  [DailyNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero);[DailyNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
 }
 Pause 400
}
function Checkbox([string]$name) {
 $condition=[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,$name)
 $item=(Main).FindFirst([Windows.Automation.TreeScope]::Descendants,$condition)
 if($null -eq $item){
  (Edits)[0].SetFocus()
  for($i=0;$i -lt 28;$i++){
   [Windows.Forms.SendKeys]::SendWait('{TAB}');Pause 50
   if([Windows.Automation.AutomationElement]::FocusedElement.Current.Name -eq $name){break}
  }
  $item=(Main).FindFirst([Windows.Automation.TreeScope]::Descendants,$condition)
 }
 Assert ($null -ne $item) "Checkbox missing: $name"
 $item.SetFocus();Pause 100
 return $item
}
function Toggle([string]$name) {
 (Checkbox $name).GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Toggle();Pause 150
}
function Capture([string]$name) {
 $rect=if($script:lastRect){$script:lastRect}else{(Main).Current.BoundingRectangle}
 $screen=[Windows.Forms.Screen]::PrimaryScreen.Bounds
 $left=[Math]::Max(0,[int]$rect.X);$top=[Math]::Max(0,[int]$rect.Y)
 $bitmap=[Drawing.Bitmap]::new([Math]::Min([int]$rect.Width,$screen.Width-$left),[Math]::Min([int]$rect.Height,$screen.Height-$top))
 $g=[Drawing.Graphics]::FromImage($bitmap)
 try{$g.CopyFromScreen($left,$top,0,0,$bitmap.Size);$bitmap.Save((Join-Path $DataDir $name),[Drawing.Imaging.ImageFormat]::Png)}finally{$g.Dispose();$bitmap.Dispose()}
}
try {
 if($LayoutOnly){
  StartApp;Unlock 'Cross-Backend-Test!2026'
  $h=[IntPtr](Main).Current.NativeWindowHandle;$scale=[DailyNative]::GetDpiForWindow($h)/96.0
  [DailyNative]::SetWindowPos($h,[IntPtr]::Zero,0,0,[int](680*$scale),[int](700*$scale),4)|Out-Null;Pause 600
  $rows=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) | Where-Object {$_.Current.Name.Contains('，') -and $_.Current.BoundingRectangle.Height -gt 20})
  $first=$rows[0].Current.BoundingRectangle
  [DailyNative]::SetCursorPos([int]($first.X+80),[int]($first.Y+$first.Height/2))|Out-Null
  [DailyNative]::mouse_event(0x800,0,0,4294966576,[UIntPtr]::Zero);Pause 600
  $window=(Main).Current.BoundingRectangle
  $rows=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) | Where-Object {$_.Current.Name.Contains('，') -and $_.Current.BoundingRectangle.Height -gt 20 -and $_.Current.BoundingRectangle.Top -gt $window.Top+240*$scale -and $_.Current.BoundingRectangle.Bottom -lt $window.Bottom-160*$scale})
  Assert ($rows.Count -gt 0) 'No fully visible scrolled row in narrow window'
  $title=($rows[0].Current.Name -replace '^已选择，','').Split('，')[0]
  RightClick $rows[0]
  $menu=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '账号右键菜单')[0].Current.BoundingRectangle
  Assert ($menu.Left -ge $window.Left -and $menu.Right -le $window.Right -and $menu.Bottom -le $window.Bottom) 'Context menu outside narrow window'
  Assert ((Row $title).Current.Name.StartsWith('已选择，')) 'Scrolled right-click selected wrong row'
  Menu '编辑账号'
  Assert ((Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq $title) 'Scrolled menu edited wrong account'
  Button '取消编辑';Button '设置'
  $save=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '保存设置')[0].Current.BoundingRectangle
  Assert ($save.Width -gt 0 -and $save.Bottom -le $window.Bottom -and $save.Top -ge $window.Top) 'Unified save not visible in narrow settings'
  Capture 'settings-narrow.png'
  Write-Output 'PASS: minimum-size scrolled account menu targets correct row, stays within window and unified save stays visible.'
  return
 }
 StartApp
 SetEdit 0 'GUI-Master-Only!2026';SetEdit 1 'GUI-Master-Only!2026';Button '创建密码库';Pause 1500;Maximize
 Button '新增账号';SetEdit 0 'Alpha';SetEdit 1 'alpha.user';SetEdit 2 'Alpha-secret!2026';Button '保存账号'
 Button '编辑账号';SetEdit 0 'Alpha V2';Button '保存账号'
 Button '新增账号';SetEdit 0 'Beta';SetEdit 1 'beta.user';SetEdit 2 'Beta-secret!2026';Button '保存账号'
 (Row 'Alpha V2').GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();Pause 150
 Button '编辑账号';SetEdit 0 'Alpha V3';Button '保存账号'
 (Row 'Alpha V3').GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();Pause 150
 RightClick (Row 'Alpha V3')
 $firstMenu=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '账号右键菜单')[0].Current.BoundingRectangle
 RightClick (Row 'Beta')
 $secondMenu=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '账号右键菜单')[0].Current.BoundingRectangle
 Assert ([Math]::Abs($secondMenu.Y-$firstMenu.Y) -gt 20) 'Menu stayed at previous right-click position'
 Assert ((Row 'Beta').Current.Name.StartsWith('已选择，')) 'Right-click did not move selected-row highlight'
 Menu '编辑账号'
 Assert ((Edits)[1].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'beta.user') 'Context menu edited the previously selected account'
 Button '取消编辑'
 RightClick (Row 'Beta');Menu '删除账号'
 Assert (TextExists '2 个账号') 'Context delete bypassed confirmation'
 Assert (MatchText '再次点击确认删除「Beta」') 'Context delete targeted wrong account'
 Button '取消删除'
 RightClick (Row 'Beta');Menu '新增账号'
 Assert ((Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq '') 'Context new reused account fields'
 Button '取消编辑'
 SetEdit 0 'no-matching-account'
 $empty=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '没有匹配的账号，请调整关键词或筛选条件。')[0]
 RightClick $empty;Menu '新增账号';Button '取消编辑';SetEdit 0 ''
 Button '新增账号';SetEdit 0 'Sensitive';SetEdit 1 'protected.user';SetEdit 2 'Sensitive-secret!2026';Toggle '敏感项：使用、查看和编辑前再次验证';Button '保存账号'
 RightClick (Row '敏感 · Sensitive');Menu '编辑账号'
 Assert (TextExists '敏感项验证') 'Context edit bypassed sensitive verification'
 Button '密码库主密码';SetEdit 0 'wrong';Button '验证主密码';Pause 700
 Assert (TextExists '敏感项验证') 'Context edit accepted wrong verification password'
 SetEdit 0 'GUI-Master-Only!2026';Button '验证主密码';Pause 900
 Assert (TextExists '账号信息') 'Context edit did not accept correct verification password'
 Button '取消编辑'
 RightClick (Row '敏感 · Sensitive');Menu '删除账号';Button '确认删除'
 Assert (TextExists '敏感项验证') 'Context delete bypassed sensitive verification'
 Button '取消验证'
 Write-Output 'PASS: right-click uses clicked account; edit/delete/new and blank-area new work; confirmation and sensitive verification remain required.'

 Button '设置';Button '本地备份列表'
 Assert (MatchText '^共 2 份') 'Backup list count wrong'
 [Windows.Forms.SendKeys]::SendWait('{DOWN}');Pause 150
 $selected=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) | Where-Object {$_.Current.Name -like '已选择，自动备份*'})
 $oldest=@(Get-ChildItem (Join-Path $DataDir 'backups') -Filter 'auto-*.db' | Sort-Object Name)[0]
 Assert ($selected.Count -eq 1 -and $selected[0].Current.Name.Contains($oldest.Name)) 'Arrow key did not select older backup'
 [Windows.Forms.SendKeys]::SendWait('{UP}');Pause 150
 Capture 'backup-list.png'
 $files=@(Get-ChildItem (Join-Path $DataDir 'backups') -Filter 'auto-*.db' | Sort-Object Name)
 Button '删除选中备份'
 Assert (@(Get-ChildItem (Join-Path $DataDir 'backups') -Filter 'auto-*.db').Count -eq 2) 'First delete click removed backup'
 Button '取消备份操作'
 Button '删除选中备份';SetEdit 0 'wrong';Button '确认删除此备份' 0;WaitDone
 Assert (MatchText '^共 2 份') 'Wrong master deleted backup'
 Button '删除选中备份';SetEdit 0 'GUI-Master-Only!2026';Button '确认删除此备份' 0;WaitDone
 Assert (MatchText '^共 1 份') 'Single delete did not refresh list'
 Assert (Test-Path $files[0].FullName) 'Newest-first list deleted the oldest backup'
 Assert (-not (Test-Path $files[1].FullName)) 'Selected newest backup still exists'
 Button '恢复选中备份';SetEdit 0 'wrong';Button '确认恢复此备份' 0;WaitDone
 Assert (MatchText '^共 1 份') 'Wrong backup password left list'
 Assert (-not (Test-Path (Join-Path $DataDir 'last-vault.txt.tmp'))) 'Unexpected preference side effect'
 Button '恢复选中备份';SetEdit 0 'GUI-Master-Only!2026';Button '确认恢复此备份' 0;WaitDone
 Assert (TextExists '1 个账号') 'Verified restore did not enter independent unlocked copy'
 Assert (TextExists 'Alpha') 'Restore did not preserve original record version'
 $restored=[IO.File]::ReadAllText((Join-Path $DataDir 'last-vault.txt'))
 Assert ($restored -ne (Join-Path $DataDir 'vault.db')) 'Restore overwrote original vault'
 Assert (Test-Path (Join-Path $DataDir 'vault.db')) 'Original vault missing'
 Assert (Test-Path $files[0].FullName) 'Restore removed original backup'
 Write-Output 'PASS: newest-first backup list, confirmed single delete, wrong password refusal, retry in list and verified independent restore without second password prompt.'

 Button '设置'
 Capture 'launch-settings.png'
 $checkbox=Checkbox '开机启动时留在托盘'
 Assert ($checkbox.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Current.ToggleState -eq [Windows.Automation.ToggleState]::Off) 'Tray default is not off'
 Toggle '开机启动时留在托盘';Button '保存设置'
 Assert ([IO.File]::ReadAllText((Join-Path $DataDir 'launch-settings.txt')) -eq "tray=1`n") 'Tray setting not persisted'
 StopApp
 $script:process=Start-Process -FilePath $ExePath -ArgumentList '--autostart' -PassThru -RedirectStandardError ($DataDir+'\stderr.log') -RedirectStandardOutput ($DataDir+'\stdout.log')
 $null=$script:process.Handle
 $events=[IntPtr]::Zero
 for($i=0;$i -lt 60;$i++){Pause 150;$events=[VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents');if($events -ne [IntPtr]::Zero){break}}
 Assert (-not $script:process.HasExited -and $events -ne [IntPtr]::Zero) 'Background start failed'
 $mainCondition=[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'Keyisle · 账号管理')
 $visibleMain=@([Windows.Automation.AutomationElement]::RootElement.FindAll([Windows.Automation.TreeScope]::Children,$mainCondition) | Where-Object {$_.Current.ProcessId -eq $script:process.Id -and -not $_.Current.IsOffscreen})
 Assert ($visibleMain.Count -eq 0) 'Tray start unexpectedly showed main window'
 [VaultTestNative]::PostMessage($events,0x8001,[IntPtr]0,[IntPtr]0x202) | Out-Null;Pause 700
 Assert (TextExists '解锁密码库') 'Tray open did not show locked vault'
 Unlock;Maximize
 Button '设置';Toggle '开机启动时留在托盘';Button '保存设置';Button '返回账号列表'
 StopApp;StartApp;Unlock
 Assert (TextExists '1 个账号') 'Restored vault selection did not survive normal restart'
 $runAfter=(Get-ItemProperty -LiteralPath $runKey -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager
 Assert ($runAfter -eq $runBefore) 'Tray-only test changed real Windows autostart entry'
 Write-Output 'PASS: tray preference defaults off, persists, starts hidden and locked, tray message opens management, normal start restores and real Run key remains unchanged.'
} finally {
 StopApp
 [DailyNative]::SetCursorPos($cursor.X,$cursor.Y) | Out-Null
 $env:PASSWORD_MANAGER_DATA_DIR=$previous
}
