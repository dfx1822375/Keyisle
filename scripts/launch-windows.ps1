param([string]$ExePath,[string]$DataDir)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies 'System.Windows.Forms.dll' @'
using System;using System.Runtime.InteropServices;using System.Windows.Forms;
public class KeyisleReceiver:Form {
 public int Escapes=0;
 protected override bool ProcessDialogKey(Keys key){if(key==Keys.Escape){Escapes++;return true;}return base.ProcessDialogKey(key);}
}
public static class KeyisleNative {
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr context);
}
'@
[KeyisleNative]::SetProcessDpiAwarenessContext([IntPtr](-4))|Out-Null
function Pause([int]$ms=250){$w=[Diagnostics.Stopwatch]::StartNew();while($w.ElapsedMilliseconds -lt $ms){[Windows.Forms.Application]::DoEvents();Start-Sleep -Milliseconds 10}}
function Popup {
 $c=[Windows.Automation.AndCondition]::new([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,$script:process.Id),[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'Keyisle · 快速填充'))
 $root=[Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$c);Assert ($null -ne $root) 'Popup missing';return $root
}
function Named([string]$name,$root=(Main)){$items=@(Visible $root ([Windows.Automation.AutomationElement]::NameProperty) $name);Assert ($items.Count -gt 0) "Missing control: $name";return $items[0]}
function Value([string]$name){return (Named $name).GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value}
function SearchPopup([string]$text){$edit=@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Edit))[0];$edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($text);Pause 150}
function SelectedPopup{$rows=@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button)|Where-Object{$_.Current.Name.StartsWith('已选择，')});Assert ($rows.Count -eq 1) 'No selected result';return $rows[0].Current.Name}
function Key([byte]$key,[bool]$down=$true){[VaultTestNative]::keybd_event($key,0,$(if($down){0}else{2}),[UIntPtr]::Zero)}
function Escape{Key 0x1b;Key 0x1b $false;Pause 350}
function OpenPopup {
 $form.Show();$form.Activate();$receiver.Focus()|Out-Null;[VaultTestNative]::SetForegroundWindow($form.Handle)|Out-Null;Pause 100
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $form.Handle) 'Receiver lost foreground'
 foreach($key in @(0x11,0x12,0x50)){Key $key};foreach($key in @(0x50,0x12,0x11)){Key $key $false};Pause 400
 $root=Popup;Assert ([VaultTestNative]::GetForegroundWindow() -eq [IntPtr]$root.Current.NativeWindowHandle) 'Popup not foreground'
 $close=Named '关闭悬浮框' $root;$bounds=$root.Current.BoundingRectangle;$button=$close.Current.BoundingRectangle
 Assert ($button.Width -lt $bounds.Width/5 -and $button.Left -gt $bounds.Left+$bounds.Width*0.8 -and $button.Right -le $bounds.Right) 'Popup close button geometry overlaps header'
 return [IntPtr]$root.Current.NativeWindowHandle
}
function Manage{$h=[VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents');[VaultTestNative]::PostMessage($h,0x8001,[IntPtr]::Zero,[IntPtr]0x202)|Out-Null;Pause 400}
function Capture([string]$name,$root=(Main)){$r=$root.Current.BoundingRectangle;$b=[Drawing.Bitmap]::new([int]$r.Width,[int]$r.Height);$g=[Drawing.Graphics]::FromImage($b);try{$g.CopyFromScreen([int]$r.Left,[int]$r.Top,0,0,$b.Size);$b.Save((Join-Path $DataDir $name),[Drawing.Imaging.ImageFormat]::Png)}finally{$g.Dispose();$b.Dispose()}}
Assert ($DataDir -match 'test-v1\.0\.0-') 'Use an isolated first-release profile'
Assert (Test-Path (Join-Path $DataDir 'vault.db')) 'Requires --seed-context-fixture'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing app before testing'
$env:PASSWORD_MANAGER_DATA_DIR=$DataDir;$script:process=$null
$form=[KeyisleReceiver]::new();$form.Text='Keyisle isolated receiver';$form.Width=450;$form.Height=180;$receiver=[Windows.Forms.TextBox]::new();$receiver.Width=390;$receiver.Top=30;$receiver.Left=20;$form.Controls.Add($receiver)
trap{Write-Output $_.Exception.Message;Write-Output $_.ScriptStackTrace;try{Capture 'failure.png'}catch{};throw}
try{
 StartApp;Button '关于';Assert (TextExists '关于 Keyisle') 'About page missing';Assert (TextExists '版本 1.0.0') 'About version missing';Capture '00-about.png';Button '返回';Unlock 'Cross-Backend-Test!2026';Capture '01-main.png'
 SetEdit 0 'atrust';Button '编辑账号';SetEdit 5 'javascript:alert(1)';SetEdit 6 '只读详情，中文备注。';Button '保存账号'
 Button '查看详情';Assert (TextExists '账号详情') 'Details missing';Assert ((Value '详情用户名') -eq 'demo.atrust') 'Wrong detail account';Assert ((Value '详情备注') -eq '只读详情，中文备注。') 'Notes missing'
 Assert ((Named '详情密码').Current.IsPassword) 'Details password is unmasked by default'
 $user=Named '详情用户名';$user.SetFocus();[VaultTestNative]::SetForegroundWindow([IntPtr](Main).Current.NativeWindowHandle)|Out-Null;[Windows.Forms.SendKeys]::SendWait('^achanged');Pause 150
 Assert ((Value '详情用户名') -eq 'demo.atrust') 'Readonly details accepted edits'
 Button '打开网址';Assert (TextExists '只支持打开完整的 HTTP 或 HTTPS 网址，请编辑此账号检查网址。') 'Unsafe URL was not refused';Capture '02-details.png'
 Button '编辑此账号';Assert ((Edits)[1].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'demo.atrust') 'Readonly view changed stored account';Button '取消编辑'
 Write-Output 'PASS: selected account opens readonly details; password masks by default, user/notes stay intact, unsafe URL is refused and edit uses the same record.'
 Button '收起';$h=OpenPopup
 Assert ((SelectedPopup) -like '*aTrust*') 'Application recency did not beat a newer global Chrome fill'
 SearchPopup 'Chrome';Assert ((SelectedPopup) -like '*Chrome*') 'Application recency overrode query relevance'
 Escape;Manage;StopApp;StartApp;Unlock 'Cross-Backend-Test!2026';Button '收起';$h=OpenPopup
 Assert ((SelectedPopup) -like '*aTrust*') 'Application usage did not survive restart';Capture '03-popup.png' (Popup)
 Write-Output 'PASS: application-specific recent accounts lead empty search across restart; explicit query relevance remains first.'
 Escape;Assert (-not [KeyisleNative]::IsWindowVisible($h)) 'Focused Escape did not close popup'
 $h=OpenPopup;$form.Activate();$receiver.Focus()|Out-Null;[VaultTestNative]::SetForegroundWindow($form.Handle)|Out-Null;Pause 100
 $before=$form.Escapes;Escape;Assert (-not [KeyisleNative]::IsWindowVisible($h)) 'Unfocused Escape did not close popup';Assert ($form.Escapes -eq $before) 'Popup dismissal leaked Escape to external app'
 $receiver.Focus()|Out-Null;Escape;Assert ($form.Escapes -eq $before+1) 'Escape hook remained active after dismissal'
 $h=OpenPopup;$form.Activate();$receiver.Focus()|Out-Null;[VaultTestNative]::SetForegroundWindow($form.Handle)|Out-Null;Pause 100
 $before=$form.Escapes;Key 0x1b;Pause 250;Key 0x1b;Pause 60;Key 0x1b $false;Pause 150
 Assert (-not [KeyisleNative]::IsWindowVisible($h)) 'Held Escape left popup open';Assert ($form.Escapes -eq $before) 'Held Escape leaked repeats'
 Escape;Assert ($form.Escapes -eq $before+1) 'Held Escape cleanup swallowed a later key'
 Assert ($receiver.Text -eq '') 'Escape or selection unexpectedly filled credentials'
 $h=OpenPopup;(Named '关闭悬浮框' (Popup)).GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();Pause 200
 Assert (-not [KeyisleNative]::IsWindowVisible($h)) 'Popup close button did not dismiss'
 Write-Output 'PASS: Escape dismisses focused or unfocused popup, suppresses the matching press/release and repeats, then restores normal Escape outside the app; no credentials are sent.'
 Manage;SetEdit 0 '';Button '新增账号';SetEdit 0 'Protected details';SetEdit 1 'protected.user';SetEdit 2 'Protected-Only!2026'
 $checkbox=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::CheckBox))[0];$checkbox.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern).Toggle();Button '保存账号'
 Button '查看详情';Assert (TextExists '敏感项验证') 'Sensitive details bypassed verification';Assert (-not (TextExists '账号详情')) 'Sensitive details leaked before verification';Button '取消验证'
 Button '查看详情';Button '密码库主密码';SetEdit 0 'Cross-Backend-Test!2026';Button '验证主密码';Pause 1300
 Assert (TextExists '账号详情') 'Verified sensitive view did not open';Assert ((Value '详情用户名') -eq 'protected.user') 'Sensitive detail wrong';Capture '04-sensitive-details.png'
 Button '编辑此账号';Assert (TextExists '敏感项验证') 'View approval authorized editing';Button '取消验证'
 $events=[VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents');[VaultTestNative]::PostMessage($events,0x2b1,[IntPtr]7,[IntPtr]::Zero)|Out-Null;Pause 300
 Assert (TextExists '解锁密码库') 'Session lock did not close details';Assert (-not (TextExists '账号详情')) 'Locked detail remained visible'
 Write-Output 'PASS: sensitive view verifies independently, cannot authorize editing, and closes with secrets cleared on session lock.'
}finally{StopApp;$form.Dispose()}
