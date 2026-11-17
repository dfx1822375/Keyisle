param([Parameter(Mandatory=$true)][string]$ExePath,[Parameter(Mandatory=$true)][string]$DataDir)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
trap { Write-Output $_.Exception.Message; Write-Output $_.ScriptStackTrace; throw }
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies 'System.Windows.Forms.dll','System.Drawing.dll' @'
using System;
using System.Runtime.InteropServices;
public static class FeedbackNative {
 [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
 [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h,IntPtr dc);
 [DllImport("gdi32.dll")] public static extern uint GetPixel(IntPtr dc,int x,int y);
 [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr LoadKeyboardLayout(string name,uint flags);
 [DllImport("user32.dll")] public static extern IntPtr GetKeyboardLayout(uint thread);
 [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint process);
 [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int command);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern bool GetCursorPos(out System.Drawing.Point p);
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
 [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h,IntPtr dc,uint flags);
 [DllImport("user32.dll")] public static extern bool RegisterHotKey(IntPtr h,int id,uint modifiers,uint key);
 [DllImport("user32.dll")] public static extern bool UnregisterHotKey(IntPtr h,int id);
}
'@
function Pause([int]$ms=300) {
 $watch=[Diagnostics.Stopwatch]::StartNew()
 while ($watch.ElapsedMilliseconds -lt $ms) { [Windows.Forms.Application]::DoEvents(); Start-Sleep -Milliseconds 15 }
}
function SendChecked([IntPtr]$handle,[string]$keys) {
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $handle) 'Foreground changed; refusing to type'
 SendTestKeys $keys; Pause 200
}
function Popup {
 $condition=[Windows.Automation.AndCondition]::new(
  [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,$script:process.Id),
  [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'Keyisle · 快速填充'))
 $item=[Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$condition)
 Assert ($null -ne $item) 'Popup missing'
 return $item
}
function SearchPopup([string]$query) {
 $root=Popup
 $edit=@(Visible $root ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Edit))[0]
 $edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($query)
 Pause 150
}
function OpenPopup([string]$keys='^%p') {
 $receiver.Clear(); $otherBox.Clear(); $form.Show(); $form.Activate(); $receiver.Focus() | Out-Null
 [VaultTestNative]::SetForegroundWindow($form.Handle) | Out-Null; Pause 200
 SendChecked $form.Handle $keys
 $popup=Popup; $h=[IntPtr]$popup.Current.NativeWindowHandle
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $h) 'Shortcut failed to activate popup'
 return $h
}
function ClickSearch {
 $edit=@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Edit))[0]
 $bounds=$edit.Current.BoundingRectangle
 [FeedbackNative]::SetCursorPos([int]($bounds.Left+40),[int]($bounds.Top+$bounds.Height/2)) | Out-Null
 [FeedbackNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
 [FeedbackNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
 Pause 300
}
function RecordShortcut([string]$keys) {
 Button '录制快捷键'
 $h=[IntPtr](Main).Current.NativeWindowHandle
 [VaultTestNative]::SetForegroundWindow($h) | Out-Null; Pause 100
 $modifiers=@()
 if($keys.Contains('^')){$modifiers+=0x11}
 if($keys.Contains('%')){$modifiers+=0x12}
 if($keys.Contains('+')){$modifiers+=0x10}
 $primary=$keys -replace '[\^%+]',''
 $vk=if($primary -match '^\{F(\d+)\}$'){0x6f+[int]$Matches[1]}else{[int][char]$primary.ToUpperInvariant()}
 foreach($key in @($modifiers)+@($vk)){[VaultTestNative]::keybd_event([byte]$key,0,0,[UIntPtr]::Zero);Pause 50}
 $release=@($modifiers)+@($vk);[Array]::Reverse($release)
 foreach($key in $release){[VaultTestNative]::keybd_event([byte]$key,0,2,[UIntPtr]::Zero);Pause 50}
 Pause 200
 if(-not (TextExists '录制快捷键')){ @(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text)) | ForEach-Object {Write-Output ('RECORDING STATE: '+$_.Current.Name)} }
 Assert (TextExists '录制快捷键') 'Shortcut recording did not complete'
 $expected=''
 if($keys.Contains('^')){$expected+='Ctrl+'}
 if($keys.Contains('%')){$expected+='Alt+'}
 if($keys.Contains('+')){$expected+='Shift+'}
 $expected+=$primary.Replace('{','').Replace('}','').ToUpperInvariant()
 Assert ((Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq $expected) ('Recorded shortcut differs: '+$expected)

}
function CaptureMain {
 $item=Main; $bounds=$item.Current.BoundingRectangle
 $bitmap=[Drawing.Bitmap]::new([int]$bounds.Width,[int]$bounds.Height)
 $graphics=[Drawing.Graphics]::FromImage($bitmap); $dc=$graphics.GetHdc()
 try { Assert ([FeedbackNative]::PrintWindow([IntPtr]$item.Current.NativeWindowHandle,$dc,2)) 'Cannot capture own window' }
 finally { $graphics.ReleaseHdc($dc); $graphics.Dispose() }
 return $bitmap
}
function CheckRestore([IntPtr]$handle,[Drawing.Bitmap]$before) {
 for($round=0;$round -lt 12;$round++) {
  [FeedbackNative]::ShowWindow($handle,6) | Out-Null; Pause 120
  [FeedbackNative]::ShowWindow($handle,9) | Out-Null
  [VaultTestNative]::SetForegroundWindow($handle) | Out-Null; Pause 500
  # Read real client pixels before any PrintWindow call can repair the surface.
  $dc=[FeedbackNative]::GetDC($handle)
  try {
   foreach($y in @(20,80,150,400,600)) {
    $pixel=[FeedbackNative]::GetPixel($dc,6,$y)
    $r=$pixel -band 255; $g=($pixel -shr 8) -band 255; $b=($pixel -shr 16) -band 255
    Assert ($r -ge 240 -and $g -ge 240 -and $b -ge 240) "Actual restored client pixel is blank: round=$round y=$y RGB=$r,$g,$b"
   }
  } finally { [FeedbackNative]::ReleaseDC($handle,$dc) | Out-Null }
 }
}

Assert (Test-Path (Join-Path $DataDir 'vault.db')) 'Requires --seed-feedback isolated fixture'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close app before test'
$previous=$env:PASSWORD_MANAGER_DATA_DIR; $env:PASSWORD_MANAGER_DATA_DIR=$DataDir
$form=[Windows.Forms.Form]::new(); $form.Text='Password helper feedback receiver'; $form.Width=450; $form.Height=180
$receiver=[Windows.Forms.TextBox]::new(); $receiver.SetBounds(20,25,370,30); $form.Controls.Add($receiver)
$other=[Windows.Forms.Form]::new(); $other.Text='Other feedback test window'; $other.Width=440; $other.Height=180
$otherBox=[Windows.Forms.TextBox]::new(); $otherBox.SetBounds(20,25,360,30); $other.Controls.Add($otherBox)
$cursor=[Drawing.Point]::new(0,0); [FeedbackNative]::GetCursorPos([ref]$cursor) | Out-Null
$script:process=$null
try {
 StartApp; Unlock 'Cross-Backend-Test!2026'
 $mainHandle=[IntPtr](Main).Current.NativeWindowHandle
 $before=CaptureMain
 try { CheckRestore $mainHandle $before } finally { $before.Dispose() }
 Write-Output 'PASS: management window survives twelve minimize/restore cycles (actual visible pixels) without losing painted content.'
 # Filter choices must agree with the displayed results and reset together.
 $combos=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::ComboBox))
 $combos[0].SetFocus(); [VaultTestNative]::SetForegroundWindow($mainHandle) | Out-Null; Pause 100
 SendChecked $mainHandle '{DOWN}'
 Assert (TextExists 'Feedback empty') 'Category selection failed'
 Assert (-not (TextExists 'Chrome 示例网站')) 'Category filter did not restrict results'
 Button '清除筛选'
 Assert (TextExists '7 个账号') 'Clear filters did not restore all fixture accounts'
 Assert (-not (TextExists '清除筛选')) 'Clear filters left stale filter controls'
 SetEdit 0 'Chrome'
 Assert (TextExists 'Chrome 示例网站') 'Cleared category still excludes searchable Chrome account'
 SetEdit 0 ''
 Write-Output 'PASS: classification filter and clear control agree with actual results.'
 Button '收起'
 foreach($case in @(@('Feedback empty','{ENTER}'),@('Feedback control','+{ENTER}'))) {
  $h=OpenPopup; SearchPopup $case[0]; SendChecked $h $case[1]; Pause 300
  Assert ($receiver.Text -eq '') 'Invalid credential partially filled receiver'
  Assert ([FeedbackNative]::IsWindowVisible($h)) 'Validation error closed popup'
  SearchPopup 'atrust'; SendChecked $h '{ENTER}'; Pause 500
  for($retry=0;$retry -lt 20 -and $receiver.Text -ne 'Demo-aTrust!2026';$retry++){Pause 100}
  if($receiver.Text -ne 'Demo-aTrust!2026'){
   Write-Output ('RETRY CASE: '+$case[0]+'; receiver length='+$receiver.Text.Length+'; other length='+$otherBox.Text.Length)
   if([FeedbackNative]::IsWindowVisible($h)){
    @(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text)) | ForEach-Object {Write-Output ('POPUP STATUS: '+$_.Current.Name)}
   }
  }
  Assert ($receiver.Text -eq 'Demo-aTrust!2026') 'Cannot choose another account after field error'
 }
 Write-Output 'PASS: empty/control-character errors keep original target; another account fills without reopening.'
 $h=OpenPopup; SearchPopup 'atrust'
 $other.Show(); $other.Activate(); $otherBox.Focus() | Out-Null
 [VaultTestNative]::SetForegroundWindow($other.Handle) | Out-Null; Pause 150
 ClickSearch
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $h) 'Mouse did not reactivate popup'
 SendChecked $h '{ENTER}'; Pause 500
 Assert ($receiver.Text -eq 'Demo-aTrust!2026' -and $otherBox.Text -eq '') 'Refocused popup could not fill original input'
 Write-Output 'PASS: mouse reactivation restores keyboard filling and fills only original input.'
 $h=OpenPopup; SearchPopup 'atrust'
 [uint32]$ownerProcess=0; $thread=[FeedbackNative]::GetWindowThreadProcessId($h,[ref]$ownerProcess)
 $layout=[FeedbackNative]::GetKeyboardLayout($thread)
 $english=[FeedbackNative]::LoadKeyboardLayout('00000409',0)
 try {
  [VaultTestNative]::PostMessage($h,0x50,[IntPtr]::Zero,$english) | Out-Null; Pause 200
  $other.Show(); $other.Activate(); [VaultTestNative]::SetForegroundWindow($other.Handle) | Out-Null; Pause 150
  ClickSearch
  SendChecked $h '^a'; SendChecked $h 'chrome'
  SendChecked $h '{ENTER}'; Pause 500
  Assert ($receiver.Text -eq 'Demo-Web!2026' -and $otherBox.Text -eq '') 'Typing after mouse reactivation failed with English input layout'
 } finally { [VaultTestNative]::PostMessage($h,0x50,[IntPtr]::Zero,$layout) | Out-Null }
 Write-Output 'PASS: typing and Enter after mouse reactivation work with a non-composing English input layout.'

 $h=OpenPopup; SearchPopup 'atrust'; SendChecked $h '^%p'
 $edits=@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Edit))
 Assert ($edits[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'atrust') 'Reactivation erased query'
 SendChecked $h '{ENTER}'; Pause 500
 Assert ($receiver.Text -eq 'Demo-aTrust!2026') 'Shortcut on existing popup lost target'
 Write-Output 'PASS: shortcut reactivation preserves query, selection and target.'
 $events=[VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents')
 [VaultTestNative]::PostMessage($events,0x8001,[IntPtr]::Zero,[IntPtr]0x202) | Out-Null; Pause 500
 Button '设置'
 RecordShortcut '^%q'
 Assert ([FeedbackNative]::RegisterHotKey($form.Handle,99,3,0x51)) 'Cannot reserve collision-test shortcut'
 try {
  Button '保存设置'
  Assert (-not (Test-Path (Join-Path $DataDir 'shortcut.txt'))) 'Conflicting shortcut was saved'
  Assert (TextExists '快捷键已被占用或由系统保留，原快捷键保持有效。') 'Conflict was not reported'
 } finally { [FeedbackNative]::UnregisterHotKey($form.Handle,99) | Out-Null }
 RecordShortcut '^+{F2}'; Button '保存设置'
 Assert ([IO.File]::ReadAllText((Join-Path $DataDir 'shortcut.txt')).Trim() -eq 'Ctrl+Shift+F2') 'Shortcut did not persist'
 Assert ([FeedbackNative]::RegisterHotKey($form.Handle,98,3,0x50)) 'Old shortcut was not released'
 [FeedbackNative]::UnregisterHotKey($form.Handle,98) | Out-Null
 [IO.Directory]::CreateDirectory((Join-Path $DataDir 'shortcut.tmp')) | Out-Null
 try {
  RecordShortcut '^%+{F9}'; Button '保存设置'
  Assert ([IO.File]::ReadAllText((Join-Path $DataDir 'shortcut.txt')).Trim() -eq 'Ctrl+Shift+F2') 'Failed save replaced original shortcut'
 } finally { [IO.Directory]::Delete((Join-Path $DataDir 'shortcut.tmp')) }
 $files=@('shortcut.txt','lock-settings.txt','launch-settings.txt')
 $snapshots=@($files | ForEach-Object {[IO.File]::ReadAllText((Join-Path $DataDir $_))})
 [IO.Directory]::CreateDirectory((Join-Path $DataDir 'lock-settings.tmp')) | Out-Null
 try {
  RecordShortcut '^%+{F9}';Button '保存设置'
  for($i=0;$i -lt $files.Count;$i++){Assert ([IO.File]::ReadAllText((Join-Path $DataDir $files[$i])) -eq $snapshots[$i]) ('Partial save changed '+$files[$i])}
  Assert ([FeedbackNative]::RegisterHotKey($form.Handle,96,7,0x78)) 'Failed save leaked prepared hotkey'
  [FeedbackNative]::UnregisterHotKey($form.Handle,96) | Out-Null
 } finally {[IO.Directory]::Delete((Join-Path $DataDir 'lock-settings.tmp'))}
 Button '返回账号列表'; Button '收起'
 $h=OpenPopup '^+{F2}'; SearchPopup 'atrust'; SendChecked $h '{ENTER}'; Pause 500
 Assert ($receiver.Text -eq 'Demo-aTrust!2026') 'Custom functional shortcut did not fill'
 Write-Output 'PASS: conflict and persistence failure retain old shortcut; successful change releases old key and fills via new key.'
 StopApp; StartApp; Unlock 'Cross-Backend-Test!2026'; Button '收起'
 $h=OpenPopup '^+{F2}'; SearchPopup 'chrome'; SendChecked $h '{ENTER}'; Pause 500
 Assert ($receiver.Text -eq 'Demo-Web!2026') 'Custom shortcut lost after restart'
 Write-Output 'PASS: custom shortcut remains effective after restart.'
 StopApp
 Assert ([FeedbackNative]::RegisterHotKey($form.Handle,97,6,0x71)) 'Cannot reserve startup conflict key'
 try {
  StartApp
  Assert (TextExists '呼出快捷键被占用，请从托盘打开主界面，在设置中修改。') 'Startup conflict prevented management access'
  Unlock 'Cross-Backend-Test!2026'; Button '设置'; RecordShortcut '^%p'; Button '保存设置'
  Assert ([IO.File]::ReadAllText((Join-Path $DataDir 'shortcut.txt')).Trim() -eq 'Ctrl+Alt+P') 'Cannot recover from startup conflict'
 } finally { [FeedbackNative]::UnregisterHotKey($form.Handle,97) | Out-Null }
 Button '返回账号列表'; Button '收起'
 $h=OpenPopup; SearchPopup 'chrome'; SendChecked $h '{ENTER}'; Pause 500
 Assert ($receiver.Text -eq 'Demo-Web!2026') 'Recovered shortcut did not fill'
 Write-Output 'PASS: startup hotkey conflict leaves management usable; settings recover a functional shortcut.'

} finally {
 StopApp
 $form.Dispose(); $other.Dispose()
 [FeedbackNative]::SetCursorPos($cursor.X,$cursor.Y) | Out-Null
 $env:PASSWORD_MANAGER_DATA_DIR=$previous
}
