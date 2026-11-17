param([Parameter(Mandatory=$true)][string]$ExePath,[Parameter(Mandatory=$true)][string]$DataDir)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms @'
using System;using System.Runtime.InteropServices;using System.Windows.Forms;
public static class PolishNative {
 [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint key,uint kind);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
 [DllImport("user32.dll")] public static extern bool RegisterHotKey(IntPtr h,int id,uint modifiers,uint key);
 [DllImport("user32.dll")] public static extern bool UnregisterHotKey(IntPtr h,int id);
 [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
 [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h,IntPtr dc);
 [DllImport("gdi32.dll")] public static extern uint GetPixel(IntPtr dc,int x,int y);
}
public class PolishReceiver:Form {
 public int Hotkeys;
 protected override void WndProc(ref Message msg){if(msg.Msg==0x312)Hotkeys++;base.WndProc(ref msg);}
}
'@
function Pause([int]$ms=300){$watch=[Diagnostics.Stopwatch]::StartNew();while($watch.ElapsedMilliseconds -lt $ms){[Windows.Forms.Application]::DoEvents();Start-Sleep -Milliseconds 10}}
function Capture([string]$name,$root=(Main)) {
 $r=$root.Current.BoundingRectangle;$screen=[Windows.Forms.Screen]::PrimaryScreen.Bounds
 $bitmap=[Drawing.Bitmap]::new([Math]::Min([int]$r.Width,$screen.Width-[int]$r.X),[Math]::Min([int]$r.Height,$screen.Height-[int]$r.Y));$graphics=[Drawing.Graphics]::FromImage($bitmap)
 try{$graphics.CopyFromScreen([int]$r.X,[int]$r.Y,0,0,$bitmap.Size);$bitmap.Save((Join-Path $DataDir $name),[Drawing.Imaging.ImageFormat]::Png)}finally{$graphics.Dispose();$bitmap.Dispose()}
}
function Row([string]$title){
 $items=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) | Where-Object {$_.Current.Name -like "*，$title，*" -or $_.Current.Name -like "$title，*"})
 Assert ($items.Count -eq 1) "Account row missing: $title";return $items[0]
}
function Toggle([string]$name){
 $item=(Main).FindFirst([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,$name))
 if($null -eq $item){
  (Edits)[0].SetFocus()
  for($i=0;$i -lt 24;$i++){
   Send '{TAB}'
   if([Windows.Automation.AutomationElement]::FocusedElement.Current.Name -eq $name){break}
  }
  $item=(Main).FindFirst([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,$name))
 }
 Assert ($null -ne $item) "Checkbox missing: $name";$item.SetFocus();Pause 150
 Assert (-not $item.Current.IsOffscreen) "Focused checkbox did not scroll into view: $name"
 $item.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Toggle();Pause 100
}
function Popup {
 $filter=[Windows.Automation.AndCondition]::new([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,$script:process.Id),[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'Keyisle · 快速填充'))
 $root=[Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$filter)
 Assert ($null -ne $root) 'Popup missing';return $root
}
function SearchPopup([string]$text){$edit=@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Edit))[0];$edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($text);$edit.SetFocus();[VaultTestNative]::SetForegroundWindow([IntPtr](Popup).Current.NativeWindowHandle)|Out-Null;Pause 300}
function Send([string]$keys,$root=(Main)){
 Assert ([VaultTestNative]::GetForegroundWindow() -eq [IntPtr]$root.Current.NativeWindowHandle) 'Foreground changed; refusing to type'
 SendTestKeys $keys;Pause 150
}
function Record {
 Button '录制快捷键'
 for($i=0;$i -lt 40 -and -not (TextExists '请按快捷键组合；Esc 取消。已占用的组合也可录入。');$i++){Pause 50}
 Assert (TextExists '请按快捷键组合；Esc 取消。已占用的组合也可录入。') 'Recorder not ready'
}
function Manage {$events=[VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents');[VaultTestNative]::PostMessage($events,0x8001,[IntPtr]::Zero,[IntPtr]0x202)|Out-Null;Pause 350}
function RightClick($row){$r=$row.Current.BoundingRectangle;[PolishNative]::SetCursorPos([int]($r.X+$r.Width/2),[int]($r.Y+$r.Height/2))|Out-Null;[PolishNative]::mouse_event(8,0,0,0,[UIntPtr]::Zero);[PolishNative]::mouse_event(16,0,0,0,[UIntPtr]::Zero);Pause 300}
function CheckMenuHover([string]$name,[bool]$danger){
 $menu=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '账号右键菜单')[0]
 $button=@(Visible $menu ([Windows.Automation.AutomationElement]::NameProperty) $name)[0];$r=$button.Current.BoundingRectangle
 [PolishNative]::SetCursorPos([int]($r.X+$r.Width/2),[int]($r.Y+$r.Height/2))|Out-Null;Pause 180
 $dc=[PolishNative]::GetDC([IntPtr]::Zero)
 try{$pixel=[PolishNative]::GetPixel($dc,[int]($r.X+8),[int]($r.Y+$r.Height/2));$red=$pixel -band 255;$green=($pixel -shr 8) -band 255;$blue=($pixel -shr 16) -band 255
  if($danger){Assert ($red -gt $green+5 -and $red -gt $blue+5) 'Delete hover is not highlighted'}else{Assert ($green -gt $red+4) "Menu hover did not follow pointer: $name"}
 }finally{[PolishNative]::ReleaseDC([IntPtr]::Zero,$dc)|Out-Null}
}
trap {Write-Output $_.Exception.Message;Write-Output $_.ScriptStackTrace;try{Capture 'failure.png'}catch{};throw}
Assert (-not (Test-Path (Join-Path $DataDir 'vault.db'))) 'Use an empty isolated test directory'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing app before testing'
$previous=$env:PASSWORD_MANAGER_DATA_DIR;$env:PASSWORD_MANAGER_DATA_DIR=$DataDir
New-Item -ItemType Directory -Force -Path $DataDir|Out-Null
[IO.File]::WriteAllText((Join-Path $DataDir 'appearance.txt'),'light')
$script:process=$null;$form=[PolishReceiver]::new();$form.Text='Isolated polish receiver';$form.Width=400;$form.Height=180;$receiver=[Windows.Forms.TextBox]::new();$receiver.Width=350;$form.Controls.Add($receiver)
$realRunPath='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$realRunBefore=(Get-ItemProperty -LiteralPath $realRunPath -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager
try{
 StartApp;Capture '01-create.png'
 SetEdit 0 'GUI-Master-Only!2026';SetEdit 1 'GUI-Master-Only!2026';Button '创建密码库';Pause 1300
 foreach($item in @(@('Alpha','alpha.user','Alpha-Test!2026'),@('企业邮箱','mail.user','Mail-Test!2026'),@('Zeta','zeta.user','Zeta-Test!2026'))){
  Button '新增账号';SetEdit 0 $item[0];SetEdit 1 $item[1];SetEdit 2 $item[2];SetEdit 3 '工作';SetEdit 4 '邮件,测试';Button '保存账号'
  Assert ((Row $item[0]).Current.Name.StartsWith('已选择，')) 'Save did not select the saved record'
 }
 Capture '02-accounts.png'
 Button '编辑账号';Capture '03-editor.png';SetEdit 0 'Unsaved Zeta';Button '取消编辑'
 Assert (TextExists '未保存的修改') 'Dirty editor exited without confirmation';Capture '04-unsaved.png'
 Button '继续编辑';Assert ((Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'Unsaved Zeta') 'Continue editing lost draft'
 Send '{ESC}';Assert (TextExists '未保存的修改') 'Escape discarded draft';Button '放弃修改'
 Assert ((Row 'Zeta').Current.Name.StartsWith('已选择，')) 'Discard changed saved record'
 Write-Output 'PASS: saved record selected; dirty cancel and Escape confirm; continue preserves fields and discard preserves stored data.'
 RightClick (Row 'Zeta');CheckMenuHover '编辑账号' $false;CheckMenuHover '删除账号' $true;CheckMenuHover '新增账号' $false;Capture '05-menu-hover.png';Send '{ESC}'
 Write-Output 'PASS: context menu highlight follows pointer across edit, delete and new; keyboard Escape closes it.'
 Button '收起';$form.Show();$form.Activate();$receiver.Focus()|Out-Null;[VaultTestNative]::SetForegroundWindow($form.Handle)|Out-Null;Pause 100
 foreach($key in @(0x11,0x12,0x50)){[VaultTestNative]::keybd_event($key,0,0,[UIntPtr]::Zero);Pause 50};foreach($key in @(0x50,0x12,0x11)){[VaultTestNative]::keybd_event($key,0,2,[UIntPtr]::Zero);Pause 50};Pause 250;$large=(Popup).Current.BoundingRectangle.Height
 foreach($action in @('编辑账号','删除账号','新增账号')){
  SearchPopup 'Zeta'
  $row=@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button)|Where-Object{$_.Current.Name -like '*Zeta，*'})[0]
  RightClick $row
  $menu=@(Visible (Popup) ([Windows.Automation.AutomationElement]::NameProperty) '账号右键菜单')
  Assert ($menu.Count -eq 1) 'Popup context menu missing'
  $item=@(Visible $menu[0] ([Windows.Automation.AutomationElement]::NameProperty) $action)[0]
  $item.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();Pause 500
  if($action -eq '删除账号'){Assert (TextExists '确认删除') 'Popup delete skipped confirmation';Button '取消删除'}
  else {Assert (TextExists '账号信息') 'Popup action did not open management editor';$value=(Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value;Assert ($value -eq $(if($action -eq '编辑账号'){'Zeta'}else{''})) 'Popup edited the wrong account';Button '取消编辑'}
  Assert ($receiver.Text -eq '') 'Popup management action unexpectedly filled receiver'
  Button '收起';$form.Activate();$receiver.Focus()|Out-Null;[VaultTestNative]::SetForegroundWindow($form.Handle)|Out-Null;Pause 100
  foreach($key in @(0x11,0x12,0x50)){[VaultTestNative]::keybd_event($key,0,0,[UIntPtr]::Zero);Pause 50};foreach($key in @(0x50,0x12,0x11)){[VaultTestNative]::keybd_event($key,0,2,[UIntPtr]::Zero);Pause 50};Pause 200
 }
 Assert (@((Popup).FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition) | Where-Object {$_.Current.Name -like '*powershell*Isolated polish receiver*'}).Count -gt 0) 'Popup target application label missing'
 Write-Output 'PASS: popup context menu edits the selected account, confirms deletion and creates new records in management without filling; target app is shown.'
 SearchPopup '邮件';Assert (@(Visible (Popup) ([Windows.Automation.AutomationElement]::NameProperty) 'mail.user · 标签：邮件,测试').Count -gt 0) 'Tag match source missing';Capture '06-popup-tags.png' (Popup)
 foreach($key in @(0x11,0x10,0x4d)){[VaultTestNative]::keybd_event($key,[byte][PolishNative]::MapVirtualKey($key,0),0,[UIntPtr]::Zero);Pause 50};foreach($key in @(0x4d,0x10,0x11)){[VaultTestNative]::keybd_event($key,[byte][PolishNative]::MapVirtualKey($key,0),2,[UIntPtr]::Zero);Pause 50}
 for($i=0;$i -lt 25;$i++){try{$null=Main;break}catch{Pause 100}}
 Assert (TextExists 'Keyisle') 'Ctrl+Shift+M failed to show management';Assert ($receiver.Text -eq '') 'Ctrl+Shift+M unexpectedly filled a field'
 Button '收起';$form.Activate();$receiver.Focus()|Out-Null;[VaultTestNative]::SetForegroundWindow($form.Handle)|Out-Null;Pause 150
 foreach($key in @(0x11,0x12,0x50)){[VaultTestNative]::keybd_event($key,0,0,[UIntPtr]::Zero);Pause 50};foreach($key in @(0x50,0x12,0x11)){[VaultTestNative]::keybd_event($key,0,2,[UIntPtr]::Zero);Pause 50};Pause 250
 SearchPopup 'youxiang';$small=(Popup).Current.BoundingRectangle.Height
 Assert ($large -gt $small+50) "Popup did not shrink with results: $large / $small"
 $title=@(Visible (Popup) ([Windows.Automation.AutomationElement]::NameProperty) '企业邮箱')[0].Current.BoundingRectangle
 $dc=[PolishNative]::GetDC([IntPtr]::Zero);$hits=0
 try{for($x=[int]$title.X;$x -lt [int]$title.Right;$x+=2){for($y=[int]$title.Y;$y -lt [int]$title.Bottom;$y+=2){$pixel=[PolishNative]::GetPixel($dc,$x,$y);$r=$pixel -band 255;$g=($pixel -shr 8) -band 255;$b=($pixel -shr 16) -band 255;if($r -gt $g*1.5 -and $g -gt $b+10){$hits++}}}}finally{[PolishNative]::ReleaseDC([IntPtr]::Zero,$dc)|Out-Null}
 Assert ($hits -gt 3) 'Matched Chinese text has no visible accent color'
 Capture '06-popup-pinyin.png' (Popup);Send '{ENTER}' (Popup)
 for($i=0;$i -lt 35 -and $receiver.Text -ne 'Mail-Test!2026';$i++){Pause 100}
 if($receiver.Text -ne 'Mail-Test!2026'){
  Capture 'failure-popup.png' (Popup)
  $names=@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text)|ForEach-Object{$_.Current.Name})
  throw "Styled popup could not fill original input: [$($receiver.Text)]; $($names -join '; ')"
 }
 Write-Output 'PASS: popup shrinks for a single pinyin match and fills original input.'
 Manage;SetEdit 0 'Alpha';Button '编辑账号';Toggle '敏感项：使用、查看和编辑前再次验证';Button '保存账号';Button '编辑账号'
 Assert (TextExists '当前方式：Windows 账户密码') 'Sensitive default mode changed';Capture '07-verification.png'
 Send '{TAB}';Send '{TAB}';Capture '08-verification-master.png';SetEdit 0 'GUI-Master-Only!2026';Button '验证主密码';Pause 1200
 Assert (TextExists '账号信息') 'Master verification failed';Button '取消编辑'
 Assert (-not (Row '敏感 · Alpha').Current.Name.Contains('alpha.user')) 'Sensitive username exposed by rich text'
 Button '备份管理';Assert (TextExists '备份管理') 'Main backup entry missing';Button '返回账号列表'
 Button '设置';Capture '09-settings.png'
 foreach($theme in @(@(0,'teal'),@(1,'blue'),@(2,'violet'),@(3,'graphite'))){
  $combo=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::ComboBox)|Where-Object{$_.Current.Name -eq '外观配色'})[0]
  $combo.SetFocus();Send '{UP}{UP}{UP}{UP}'
  for($i=0;$i -lt $theme[0];$i++){Send '{DOWN}'}
  Button '保存设置';Assert ((Get-Content -Raw (Join-Path $DataDir 'theme.txt')).Trim() -eq $theme[1]) "Theme save failed: $($theme[1])"
  $button=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '保存设置')[0].Current.BoundingRectangle
  $dc=[PolishNative]::GetDC([IntPtr]::Zero);try{$pixel=[PolishNative]::GetPixel($dc,[int]($button.Left+8),[int]($button.Top+$button.Height/2))}finally{[PolishNative]::ReleaseDC([IntPtr]::Zero,$dc)|Out-Null}
  $expected=@(0x5f6711,0xb05d24,0xa64370,0x645245)[$theme[0]]
  Assert ($pixel -eq $expected) "Saved theme did not recolor the actual button: $($theme[1]) pixel=$pixel expected=$expected"
  Capture ("theme-"+$theme[1]+'.png')
 }
 Write-Output 'PASS: all four themes save through the unified settings action; main backup management returns to the account list.'
 Button '返回账号列表';Button '收起';$form.Activate();$receiver.Focus()|Out-Null;[VaultTestNative]::SetForegroundWindow($form.Handle)|Out-Null;Pause 150
 foreach($key in @(0x11,0x12,0x50)){[VaultTestNative]::keybd_event($key,0,0,[UIntPtr]::Zero);Pause 50};foreach($key in @(0x50,0x12,0x11)){[VaultTestNative]::keybd_event($key,0,2,[UIntPtr]::Zero);Pause 50};Pause 250
 $row=@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button)|Where-Object{$_.Current.Name.StartsWith('已选择，')})[0].Current.BoundingRectangle
 $dc=[PolishNative]::GetDC([IntPtr]::Zero);try{Assert ([PolishNative]::GetPixel($dc,[int]($row.Left+2),[int]($row.Top+$row.Height/2)) -eq 0x645245) 'Popup did not receive the saved theme'}finally{[PolishNative]::ReleaseDC([IntPtr]::Zero,$dc)|Out-Null}
 Capture 'theme-popup-graphite.png' (Popup);Send '{ESC}' (Popup);Manage;Button '设置'


 $beforeShortcut=Get-Content -Raw (Join-Path $DataDir 'shortcut.txt')
 Assert ([PolishNative]::RegisterHotKey($form.Handle,97,3,0x51)) 'Cannot reserve occupied recording combination'
 try{
  $mainHandle=[IntPtr](Main).Current.NativeWindowHandle
  [VaultTestNative]::SetForegroundWindow($mainHandle)|Out-Null;Pause 200
  Assert ([VaultTestNative]::GetForegroundWindow() -eq $mainHandle) 'Recording window is not foreground'
  Record;foreach($key in @(0x11,0x12,0x51)){[VaultTestNative]::keybd_event($key,0,0,[UIntPtr]::Zero);Pause 50};foreach($key in @(0x51,0x12,0x11)){[VaultTestNative]::keybd_event($key,0,2,[UIntPtr]::Zero);Pause 50};Pause 250
  $recorded=(Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value
  Assert ($recorded -eq 'Ctrl+Alt+Q') "Occupied shortcut could not be recorded: $recorded"
  Assert ($form.Hotkeys -eq 0) 'Recorder triggered external shortcut'
  Button '保存设置';Assert ((Get-Content -Raw (Join-Path $DataDir 'shortcut.txt')) -eq $beforeShortcut) 'Conflicting shortcut replaced persisted settings'
  Assert (TextExists '快捷键已被占用或由系统保留，原快捷键保持有效。') 'Save did not report conflict'
 }finally{[PolishNative]::UnregisterHotKey($form.Handle,97)|Out-Null}
 Record;foreach($key in @(0x11,0x12,0x50)){[VaultTestNative]::keybd_event($key,0,0,[UIntPtr]::Zero);Pause 50};foreach($key in @(0x50,0x12,0x11)){[VaultTestNative]::keybd_event($key,0,2,[UIntPtr]::Zero);Pause 50};Pause 250;Button '保存设置'
 Toggle '开机启动时留在托盘';Button '保存设置'
 Button '返回账号列表';Button '设置'
 Button '本地备份列表';Capture '10-backups.png';Button '返回设置';Button '主密码与本地备份';Capture '11-maintenance.png';Button '返回设置';Button '返回账号列表'
 Write-Output 'PASS: occupied shortcut records without external action; save reports collision and retains old key; own shortcut records normally.'
 StopApp;StartApp;Assert (TextExists '解锁密码库') 'Manual start hid in tray with tray preference enabled'
 $button=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '解锁')[0].Current.BoundingRectangle
 $dc=[PolishNative]::GetDC([IntPtr]::Zero);try{Assert ([PolishNative]::GetPixel($dc,[int]($button.Left+8),[int]($button.Top+$button.Height/2)) -eq 0x645245) 'Saved theme did not survive restart'}finally{[PolishNative]::ReleaseDC([IntPtr]::Zero,$dc)|Out-Null}
 Capture '12-unlock.png' ;StopApp
 $script:process=Start-Process -FilePath $ExePath -ArgumentList '--autostart' -PassThru -RedirectStandardError ($DataDir+'\stderr.log') -RedirectStandardOutput ($DataDir+'\stdout.log');$null=$script:process.Handle
 $events=[IntPtr]::Zero;for($i=0;$i -lt 80;$i++){Pause 100;$events=[VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents');if($events -ne [IntPtr]::Zero){break}}
 Assert ($events -ne [IntPtr]::Zero) 'Autostart did not create tray service';Pause 400
 $windows=@([Windows.Automation.AutomationElement]::RootElement.FindAll([Windows.Automation.TreeScope]::Children,[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,$script:process.Id))|Where-Object{-not $_.Current.IsOffscreen -and $_.Current.Name -eq 'Keyisle · 账号管理'})
 Assert ($windows.Count -eq 0) 'Autostart tray preference showed main window'
 $second=Start-Process -FilePath $ExePath -PassThru
 Assert ($second.WaitForExit(12000)) 'Second instance did not hand off to existing instance';Pause 500
 Assert (TextExists '解锁密码库') 'Manual second launch did not show existing background instance'
 Write-Output 'PASS: manual launch always shows main; autostart can stay hidden; second manual launch activates existing locked instance.'
 Unlock;SetEdit 0 'Zeta';Button '编辑账号';SetEdit 0 'Dirty exit';[VaultTestNative]::PostMessage($events,0x10,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null;Pause 400
 Assert (TextExists '未保存的修改') 'Tray exit discarded editor';Button '继续编辑'
 Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq $events) 'Continue editing lost tray/event service'
 [VaultTestNative]::PostMessage($events,0x10,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null;Pause 300;Button '放弃修改并退出'
 Assert ($script:process.WaitForExit(5000)) 'Confirmed exit did not stop app'
 $realRunAfter=(Get-ItemProperty -LiteralPath $realRunPath -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager
 Assert ($realRunAfter -eq $realRunBefore) 'GUI tests modified real Windows autostart registration'
 Write-Output 'PASS: exit guard preserves native service until confirmed; real Windows autostart value unchanged.'
}catch{try{Capture 'failure.png'}catch{};throw}finally{StopApp;$form.Dispose();$env:PASSWORD_MANAGER_DATA_DIR=$previous}
