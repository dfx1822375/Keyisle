param([string]$ExePath,[string]$DataDir)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;using System.Runtime.InteropServices;
public static class ExperienceNative {
 [DllImport("user32.dll")]public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
 [DllImport("user32.dll")]public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")]public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int width,int height,uint flags);
 [DllImport("user32.dll")]public static extern bool IsZoomed(IntPtr h);
 [DllImport("user32.dll")]public static extern IntPtr GetDC(IntPtr h);
 [DllImport("user32.dll")]public static extern int ReleaseDC(IntPtr h,IntPtr dc);
 [DllImport("gdi32.dll")]public static extern uint GetPixel(IntPtr dc,int x,int y);
 [DllImport("user32.dll")]public static extern bool SetCursorPos(int x,int y);
}
'@
[ExperienceNative]::SetThreadDpiAwarenessContext([IntPtr](-4))|Out-Null
function Pause([int]$ms=300){$watch=[Diagnostics.Stopwatch]::StartNew();while($watch.ElapsedMilliseconds -lt $ms){[Windows.Forms.Application]::DoEvents();Start-Sleep -Milliseconds 10}}
function Capture([string]$name,$root=(Main)){$r=$root.Current.BoundingRectangle;$b=[Drawing.Bitmap]::new([int]$r.Width,[int]$r.Height);$g=[Drawing.Graphics]::FromImage($b);try{$g.CopyFromScreen([int]$r.X,[int]$r.Y,0,0,$b.Size);$b.Save((Join-Path $DataDir $name),[Drawing.Imaging.ImageFormat]::Png)}finally{$g.Dispose();$b.Dispose()}}
function Pixel([int]$x,[int]$y){$dc=[ExperienceNative]::GetDC([IntPtr]::Zero);try{return [ExperienceNative]::GetPixel($dc,$x,$y)}finally{[ExperienceNative]::ReleaseDC([IntPtr]::Zero,$dc)|Out-Null}}
function Red([uint32]$color){return ($color -band 255)}
function FocusAccent($inputControl) {
 $inputControl.SetFocus();Pause 220
 $bounds=$inputControl.Current.BoundingRectangle
 $bgr=Pixel ([int]($bounds.Left+40)) ([int]($bounds.Bottom-2))
 return [Drawing.Color]::FromArgb(($bgr -band 255),(($bgr -shr 8) -band 255),(($bgr -shr 16) -band 255))
}
function Canvas($root=(Main)){$r=$root.Current.BoundingRectangle;$s=[ExperienceNative]::GetDpiForWindow([IntPtr]$root.Current.NativeWindowHandle)/96.0;return Pixel ([int]($r.Left+8*$s)) ([int]($r.Top+100*$s))}
function Combo([string]$name,[int]$index){$items=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::ComboBox)|Where-Object{$_.Current.Name -eq $name});Assert ($items.Count -eq 1) "Combo missing: $name";$items[0].SetFocus();[VaultTestNative]::SetForegroundWindow([IntPtr](Main).Current.NativeWindowHandle)|Out-Null;[Windows.Forms.SendKeys]::SendWait('{UP}{UP}{UP}{UP}');for($i=0;$i -lt $index;$i++){[Windows.Forms.SendKeys]::SendWait('{DOWN}')};Pause 150}
function Popup{$condition=[Windows.Automation.AndCondition]::new([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,$script:process.Id),[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'Keyisle · 快速填充'));$r=[Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$condition);Assert ($null -ne $r) 'Popup missing';return $r}
Assert (-not (Test-Path (Join-Path $DataDir 'vault.db'))) 'Use a fresh isolated fixture'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close existing app before GUI checks'
$previous=$env:PASSWORD_MANAGER_DATA_DIR;$env:PASSWORD_MANAGER_DATA_DIR=$DataDir;New-Item -ItemType Directory -Force $DataDir|Out-Null
$script:process=$null;$receiver=[Windows.Forms.Form]::new();$receiver.Text='目标页面 · Local account input';$receiver.Width=520;$receiver.Height=200;$receiverEdit=[Windows.Forms.TextBox]::new();$receiverEdit.Width=430;$receiverEdit.Left=20;$receiverEdit.Top=40;$receiver.Controls.Add($receiverEdit)
trap {Write-Output $_.Exception.Message;Write-Output $_.ScriptStackTrace;try{Capture 'failure.png'}catch{};throw}
try{
 StartApp;Capture '01-create.png';SetEdit 0 'GUI-Master-Only!2026';SetEdit 1 'GUI-Master-Only!2026';Button '创建密码库';Pause 1500
 foreach($item in @(@('企业邮箱','mail.user'),@('aTrust 测试','vpn.user'))){Button '新增账号';SetEdit 0 $item[0];SetEdit 1 $item[1];SetEdit 2 'Example-Only!2026';Button '保存账号'}
 Button '新增账号';SetEdit 0 '空用户名回归';SetEdit 2 'Example-Only!2026';Button '保存账号'
 $h=[IntPtr](Main).Current.NativeWindowHandle;$s=[ExperienceNative]::GetDpiForWindow($h)/96.0
 [ExperienceNative]::SetWindowPos($h,[IntPtr]::Zero,120,90,[int](840*$s),[int](610*$s),4)|Out-Null;Pause 800
 $before=(Main).Current.BoundingRectangle;Assert (Test-Path (Join-Path $DataDir 'window.txt')) 'Layout not saved'
 StopApp;StartApp;Assert (TextExists '解锁密码库') 'Cold start did not stay locked';Unlock;Pause 700
 $after=(Main).Current.BoundingRectangle
 Assert ([Math]::Abs($before.Width-$after.Width) -le 3 -and [Math]::Abs($before.Height-$after.Height) -le 3) 'Cold restart lost window dimensions'
 Assert ([Math]::Abs($before.Left-$after.Left) -le 3 -and [Math]::Abs($before.Top-$after.Top) -le 3) 'Cold restart lost window position'
 Button '最大化或还原窗口';Pause 700;StopApp;StartApp;Unlock;Pause 700
 Assert ([ExperienceNative]::IsZoomed([IntPtr](Main).Current.NativeWindowHandle)) 'Cold restart lost maximized preference';Button '最大化或还原窗口'
 StopApp;$values=[IO.File]::ReadAllLines((Join-Path $DataDir 'window.txt'));$values[0]='80000';$values[1]='80000';$values[2]='81260';$values[3]='80915';[IO.File]::WriteAllLines((Join-Path $DataDir 'window.txt'),$values)
 StartApp;Unlock;Pause 700;$r=(Main).Current.BoundingRectangle;$area=[Windows.Forms.Screen]::FromHandle([IntPtr](Main).Current.NativeWindowHandle).WorkingArea
 Assert ($r.Left -ge $area.Left-2 -and $r.Top -ge $area.Top-2 -and $r.Right -le $area.Right+2 -and $r.Bottom -le $area.Bottom+2) 'Off-screen saved layout did not return to the work area'
 Capture '02-layout-restored.png'
 Write-Output 'PASS: cold restart preserves normal size/position and maximized state; missing-monitor coordinates return to the work area.'
 Button '设置';Combo '外观模式' 1;Button '保存设置';Assert ((Get-Content -Raw (Join-Path $DataDir 'appearance.txt')).Trim() -eq 'light') 'Light mode not saved'
 $light=Canvas;Assert ((Red $light) -gt 230 -and $light -ne 0xffffff) 'Light canvas did not use the soft off-white surface'
 $r=(Main).Current.BoundingRectangle;$edge=Pixel ([int]($r.Left+1)) ([int]($r.Top+$r.Height/2));Assert ([Math]::Abs((Red $edge)-(Red $light)) -gt 15) 'Window outline is not distinguishable on a white desktop'
 Capture '03-settings-light.png'
 for ($scheme=0;$scheme -lt 4;$scheme++) {
  Combo '外观配色' $scheme;Button '保存设置';Button '返回账号列表'
  $searchInput=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '搜索账号')[0]
  $accent=FocusAccent $searchInput
  $hue=$accent.GetHue()
  $matchesTheme=switch ($scheme) {
   0 {$hue -ge 135 -and $hue -le 195 -and $accent.GetSaturation() -gt 0.25}
   1 {$hue -ge 196 -and $hue -le 245 -and $accent.GetSaturation() -gt 0.25}
   2 {$hue -ge 246 -and $hue -le 295 -and $accent.GetSaturation() -gt 0.25}
   3 {$accent.GetSaturation() -lt 0.3 -and $accent.GetBrightness() -lt 0.6}
  }
  Assert $matchesTheme "Fluent focus accent did not match scheme $scheme ($accent)"
  Button '设置'
 }
 Write-Output 'PASS: Fluent focus accents follow all four application color schemes.'
 Combo '外观模式' 2;Combo '外观配色' 1;Button '保存设置';Assert ((Red (Canvas)) -lt 80) 'Dark page canvas not applied';Capture '04-settings-dark.png';Button '返回账号列表'
 $searchInput=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '搜索账号')[0]
 $search=$searchInput.Current.BoundingRectangle
 $darkAccent=FocusAccent $searchInput
 Assert ((Red (Pixel ([int]($search.Left+40)) ([int]($search.Top+6)))) -lt 110) 'Native input stayed white in dark mode'
 Capture '05-main-dark.png'
 Button '收起';$receiver.Show();$receiver.Activate();$receiverEdit.Focus()|Out-Null;[VaultTestNative]::SetForegroundWindow($receiver.Handle)|Out-Null
 $rr=$receiverEdit.Bounds;$point=$receiverEdit.PointToScreen([Drawing.Point]::new(30,12));[ExperienceNative]::SetCursorPos($point.X,$point.Y)|Out-Null;Pause 200
 [Windows.Forms.SendKeys]::SendWait('^%p');Pause 700;$pop=Popup
 Assert ((Red (Canvas $pop)) -lt 80) 'Popup did not receive dark mode'
 $texts=@(Visible $pop ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text)|ForEach-Object{$_.Current.Name})
 Assert (@($texts|Where-Object{$_ -like '*powershell*目标页面*Local account input*'}).Count -gt 0) 'Popup omitted application/window-page title'
 Assert (@($texts|Where-Object{$_ -match '^原输入框$|^原窗口 · 鼠标位置$'}).Count -gt 0) 'Popup omitted the input location source'
 $location=@(Visible $pop ([Windows.Automation.AutomationElement]::NameProperty) '位置详情')[0]
 Assert ($null -ne $location) 'Popup omitted the location detail action'
 $location.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();Pause 250
 $texts=@(Visible (Popup) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text)|ForEach-Object{$_.Current.Name})
 Assert (@($texts|Where-Object{$_ -match '(输入光标位置|呼出时鼠标) \(-?\d+, -?\d+\)'}).Count -gt 0) 'Location detail omitted captured input/callout coordinates'
 $edit=@(Visible $pop ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Edit))[0];$edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue('youxiang')
 Assert ((FocusAccent $edit).ToArgb() -eq $darkAccent.ToArgb()) 'Popup had a different Fluent accent from the main window'
 Capture '06-popup-dark-target.png' (Popup)
 $edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue('空用户名回归');$edit.SetFocus();Pause 250
 $popupBefore=(Popup).Current.BoundingRectangle
 SendTestKeys '^{ENTER}';Pause 650
 $pop=Popup
 $errorText=@(Visible $pop ([Windows.Automation.AutomationElement]::NameProperty) '字段为空或包含控制字符，已取消填充。 可选择其他账号或填充方式后重试。')[0]
 Assert ($null -ne $errorText) 'Popup did not show the failed fill feedback'
 $popupAfter=$pop.Current.BoundingRectangle
 Assert ($popupAfter.Height -eq $popupBefore.Height) 'Status changed the popup height'
 Assert ($errorText.Current.BoundingRectangle.Bottom -le $popupAfter.Bottom) 'Popup feedback is clipped'
 Capture '09-popup-status.png' $pop
 Write-Output 'PASS: fill error stays inside the compact popup without changing its height.'
 [Windows.Forms.SendKeys]::SendWait('^+m');Pause 700;Assert ($receiverEdit.Text -eq '') 'Management shortcut filled receiver'
 StopApp;StartApp;Assert ((Red (Canvas)) -lt 80) 'Dark theme did not survive cold startup';Capture '07-unlock-dark.png';Unlock
 Button '设置';Combo '外观模式' 0;Button '保存设置';$appsLight=1;try{$appsLight=(Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize' -Name AppsUseLightTheme).AppsUseLightTheme}catch{}
 Assert (((Red (Canvas)) -lt 80) -eq ($appsLight -eq 0)) 'Follow-system mode disagrees with Windows application preference'
 Capture '08-settings-system.png';Button '返回账号列表'
 Write-Output 'PASS: light/dark/system appearance, dark native inputs, popup synchronization, visible outline and cold startup theme persist.'
 Write-Output 'PASS: popup shows application plus page/window title and source-labelled position; navigation sends no credentials.'
}finally{StopApp;$receiver.Dispose();$env:PASSWORD_MANAGER_DATA_DIR=$previous}
