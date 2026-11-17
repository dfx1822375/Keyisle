param([Parameter(Mandatory=$true)][string]$ExePath, [string]$ScreenshotPath, [string]$ChromePath, [Parameter(Mandatory=$true)][string]$DataDir, [switch]$TestSensitive,[switch]$ChromeOnly)
$ErrorActionPreference = 'Stop'
$previousDataDir = $env:PASSWORD_MANAGER_DATA_DIR
$realRunPath = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$realStartupBefore = (Get-ItemProperty -LiteralPath $realRunPath -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager
$env:PASSWORD_MANAGER_DATA_DIR = $DataDir
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -ReferencedAssemblies 'System.Windows.Forms.dll','System.Drawing.dll' @'
using System;
using System.Runtime.InteropServices;
public static class SmokeNative {
    [DllImport("user32.dll")] public static extern void keybd_event(byte key,byte scan,uint flags,UIntPtr extra);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr LoadKeyboardLayout(string name, uint flags);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr h, int index);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
 }
public class DelayedTabBox : System.Windows.Forms.TextBox {
    public bool DelayTab { get; set; }
    public int DelayMilliseconds { get { return transition.Interval; } set { transition.Interval = value; } }
    public void CancelTransition() { transition.Stop(); ReadOnly = false; }
    public System.Windows.Forms.Control Destination { get; set; }
    private readonly System.Windows.Forms.Timer transition = new System.Windows.Forms.Timer();
    public DelayedTabBox() {
        transition.Interval = 250;
        transition.Tick += (sender, args) => {
            transition.Stop();
            ReadOnly = false;
            if (Destination != null && !Destination.IsDisposed) Destination.Focus();
        };
    }
    protected override bool ProcessDialogKey(System.Windows.Forms.Keys keyData) {
        if (DelayTab && keyData == System.Windows.Forms.Keys.Tab) {
            ReadOnly = true;
            transition.Start();
            return true;
        }
        return base.ProcessDialogKey(keyData);
    }
    protected override void Dispose(bool disposing) {
        if (disposing) transition.Dispose();
        base.Dispose(disposing);
    }
}
'@
function Pump([int]$ms) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while ($clock.ElapsedMilliseconds -lt $ms) {
        [Windows.Forms.Application]::DoEvents()
        Start-Sleep -Milliseconds 20
    }
}
function Assert-True($condition, [string]$message) {
    if (-not $condition) { throw $message }
}
function Send-Checked([IntPtr]$handle, [string]$keys) {
    Assert-True ([SmokeNative]::GetForegroundWindow() -eq $handle) 'Foreground changed; refusing to type.'
    $modifier=switch($keys){'{ENTER}'{0};'^{ENTER}'{0x11};'+{ENTER}'{0x10};default{-1}}
    if($modifier -lt 0){[Windows.Forms.SendKeys]::SendWait($keys)}else{
        if($modifier -ne 0){[SmokeNative]::keybd_event([byte]$modifier,0,0,[UIntPtr]::Zero)}
        [SmokeNative]::keybd_event(0x0d,0,0,[UIntPtr]::Zero);[SmokeNative]::keybd_event(0x0d,0,2,[UIntPtr]::Zero)
        if($modifier -ne 0){[SmokeNative]::keybd_event([byte]$modifier,0,2,[UIntPtr]::Zero)}
    }
    Pump 160
}
function Find-AppWindow([string]$title = 'Keyisle · 快速填充') {
    $process.Refresh()
    $condition = [Windows.Automation.AndCondition]::new(
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty, $process.Id),
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $title))
    $element = [Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children, $condition)
    Assert-True ($null -ne $element) 'Application window was not found.'
    return $element
}
function Set-Search([IntPtr]$handle, [string]$text) {
    Assert-True ([SmokeNative]::GetForegroundWindow() -eq $handle) 'Foreground changed before search.'
    $element = Find-AppWindow
    $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Edit)
    $edit = $element.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
    Assert-True ($null -ne $edit) 'Search field not exposed to accessibility.'
    $edit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($text)
    # Search is already focused; a redundant UIA request can reactivate the popup after filling.
    Pump 200
}
function Click-BrowserInput($window,$element) {
    $element.SetFocus();Pump 200
    $h=[IntPtr]$window.Current.NativeWindowHandle
    [SmokeNative]::SetForegroundWindow($h)|Out-Null
    $r=$element.Current.BoundingRectangle
    Assert-True ($r.Width -gt 0 -and $r.Height -gt 0) 'Browser input not visible'
    [SmokeNative]::SetCursorPos([int]($r.Left+$r.Width/2),[int]($r.Top+$r.Height/2))|Out-Null
    [SmokeNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero);[SmokeNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
    Pump 400
}
function Open-ForReceiver {
    $box.Clear()
    $passwordBox.Clear()
    $form.Show()
    $form.Activate()
    $box.Focus() | Out-Null
    [Windows.Automation.AutomationElement]::FromHandle($box.Handle).SetFocus()
    [SmokeNative]::SetForegroundWindow($form.Handle) | Out-Null
    Pump 300
    Send-Checked $form.Handle '^%p'
    Pump 700
    $element = Find-AppWindow
    $handle = [IntPtr]$element.Current.NativeWindowHandle
    for ($focusAttempt=0; $focusAttempt -lt 10 -and [SmokeNative]::GetForegroundWindow() -ne $handle; $focusAttempt++) { Pump 100 }
    if ([SmokeNative]::GetForegroundWindow() -ne $handle) { Write-Warning ('FOCUS: actual={0} popup={1} receiver={2} popupVisible={3}' -f [SmokeNative]::GetForegroundWindow(),$handle,$form.Handle,[SmokeNative]::IsWindowVisible($handle)) }
    Assert-True ([SmokeNative]::GetForegroundWindow() -eq $handle) 'Hotkey did not focus the search panel.'
    Assert-True ($handle -ne $script:mainHandle) 'Hotkey reused the management window.'
    Assert-True (-not [SmokeNative]::IsWindowVisible($script:mainHandle)) 'Hotkey unexpectedly showed the management window.'
    $extended = [SmokeNative]::GetWindowLongPtr($handle, -20).ToInt64()
    $titleCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::TitleBar)
    $titleBar = $element.FindFirst([Windows.Automation.TreeScope]::Children, $titleCondition)
    Assert-True ($null -eq $titleBar -or $titleBar.Current.IsOffscreen) 'Popup still exposes a visible native title bar.' 
    Assert-True (($extended -band 0x80) -ne 0 -and ($extended -band 0x40000) -eq 0) 'Popup is not a taskbar-free tool window.'
    $scale = [SmokeNative]::GetDpiForWindow($handle) / 96.0
    Assert-True (($element.Current.BoundingRectangle.Height / $scale) -le 516 -and ($element.Current.BoundingRectangle.Width / $scale) -le 604) 'Popup exceeds the five-row layout.'
    return $handle
}
function Invoke-TestButton([string]$title,[string]$name,[int]$wait=200) {
    $root = Find-AppWindow $title
    $buttons = @($root.FindAll([Windows.Automation.TreeScope]::Descendants,
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Button)) | Where-Object { -not $_.Current.IsOffscreen -and $_.Current.Name -eq $name })
    if ($buttons.Count -eq 0 -and $name -eq '收起') {
        Invoke-TestButton $title '更多'
        $buttons=@((Find-AppWindow $title).FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'收起到托盘'))|Where-Object{-not $_.Current.IsOffscreen})
    }
    if ($buttons.Count -eq 0 -and $name -eq '编辑账号') {
        $edit=@($root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'搜索账号'))|Where-Object{-not $_.Current.IsOffscreen})[0]
        $edit.SetFocus();Send-Checked ([IntPtr]$root.Current.NativeWindowHandle) '{F2}';Pump $wait;return
    }
    Assert-True ($buttons.Count -gt 0) "Test button missing: $name"
    $buttons[0].GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
    Pump $wait
}
$existing = [SmokeNative]::FindWindow('PasswordManagerPrototypeEvents', 'PasswordManagerPrototypeEvents')
Assert-True ($existing -eq [IntPtr]::Zero) 'Close the existing prototype before running the smoke test.'
$form = [Windows.Forms.Form]::new()
$form.Text = 'Password Manager - fake input receiver'
$form.Size = [Drawing.Size]::new(520, 240)
$form.StartPosition = 'CenterScreen'
$label = [Windows.Forms.Label]::new()
$label.Text = 'AUTOMATED TEST: only fake credentials are typed here.'
$label.Location = [Drawing.Point]::new(18, 18)
$label.AutoSize = $true
$box = [DelayedTabBox]::new()
$box.Location = [Drawing.Point]::new(18, 55)
$box.Width = 460
$box.Multiline = $false
$box.TabIndex = 0
$box.Height = 50
$script:submitted = $false
$box.Add_KeyDown({ if ($_.KeyCode -eq [Windows.Forms.Keys]::Enter) { $script:submitted = $true } })
$form.Controls.Add($label)
$form.Controls.Add($box)
$passwordBox = [Windows.Forms.TextBox]::new()
$passwordBox.Location = [Drawing.Point]::new(18, 110)
$passwordBox.Width = 460
$passwordBox.TabIndex = 1
$passwordBox.Add_KeyDown({ if ($_.KeyCode -eq [Windows.Forms.Keys]::Enter) { $script:submitted = $true } })
$form.Controls.Add($passwordBox)
$box.Destination = $passwordBox
$process = $null
try {
    $process = Start-Process -FilePath $ExePath -PassThru -RedirectStandardError ($ExePath + '.stderr.log') -RedirectStandardOutput ($ExePath + '.stdout.log')
    $null = $process.Handle
    $ready = $false
    for ($attempt = 0; $attempt -lt 80; $attempt++) {
        Pump 200
        Assert-True (-not $process.HasExited) 'Prototype exited on startup.'
        try { $element = Find-AppWindow 'Keyisle · 账号管理'; $ready = $true; break } catch { }
    }
    Assert-True $ready 'Prototype did not show its first window within 16 seconds.'
    Pump 500
    $editCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Edit)
    $masterEdit = @($element.FindAll([Windows.Automation.TreeScope]::Descendants, $editCondition) | Where-Object { -not $_.Current.IsOffscreen })[0]
    $masterEdit.SetFocus()
    $initialHandle = [IntPtr]$element.Current.NativeWindowHandle
    $englishLayout = [SmokeNative]::LoadKeyboardLayout('00000409', 0)
    [SmokeNative]::PostMessage($initialHandle,0x50,[IntPtr]::Zero,$englishLayout) | Out-Null
    [SmokeNative]::PostMessage($form.Handle,0x50,[IntPtr]::Zero,$englishLayout) | Out-Null
    Pump 200
    [SmokeNative]::SetForegroundWindow($initialHandle) | Out-Null
    Pump 150
    Send-Checked $initialHandle '^a'
    $masterEdit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue('Cross-Backend-Test!2026')
    $unlockCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, '解锁')
    $unlockButton = @($element.FindAll([Windows.Automation.TreeScope]::Descendants, $unlockCondition) | Where-Object { -not $_.Current.IsOffscreen })[0]
    $unlockButton.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
    Pump 1800
    Write-Output 'PASS: fixture vault unlocked for keyboard regression tests.'
    $script:mainHandle = [IntPtr]$element.Current.NativeWindowHandle
    Invoke-TestButton 'Keyisle · 账号管理' '收起'
    Pump 500
    if (-not $ChromeOnly) {
    $panel = Open-ForReceiver
    Set-Search $panel 'atrust'
    Send-Checked $panel '{ENTER}'
    Pump 800
    Assert-True ($box.Text -eq 'Demo-aTrust!2026') "Password mismatch: $($box.Text)"
    Assert-True (-not $script:submitted) 'Unexpected Enter reached the receiver.'
    Write-Output 'PASS: separate compact, frameless tool popup; Enter fills only password without submission.'

    $panel = Open-ForReceiver
    Set-Search $panel 'demo.unicode'
    Send-Checked $panel '{ENTER}'
    Pump 800
    Assert-True ($box.Text -eq '示例!@#€🔒') 'Unicode password mismatch.'
    Write-Output 'PASS: CJK, punctuation, euro and supplementary Unicode input.'

    $panel = Open-ForReceiver
    Set-Search $panel 'atrust'
    Send-Checked $panel '^{ENTER}'
    Pump 900
    Assert-True ($box.Text -eq 'demo.atrust') 'Ctrl+Enter username mismatch.'
    Assert-True ($passwordBox.Text -eq '') 'Username action unexpectedly filled the next field.'
    Write-Output 'PASS: Ctrl+Enter fills only the username.'

    $panel = Open-ForReceiver
    Set-Search $panel 'atrust'
    Send-Checked $panel '+{ENTER}'
    Pump 900
    Assert-True ($box.Text -eq 'demo.atrust') 'Combined fill username mismatch.'
    Assert-True ($passwordBox.Text -eq 'Demo-aTrust!2026') 'Combined fill password mismatch.'
    Assert-True (-not $script:submitted) 'Combined fill submitted Enter.'
    Write-Output 'PASS: Shift+Enter fills username -> Tab -> password, without submission.'

    $box.DelayTab = $true
    try {
        $panel = Open-ForReceiver
        Set-Search $panel 'atrust'
        Send-Checked $panel '+{ENTER}'
        Pump 1200
        Assert-True ($box.Text -eq 'demo.atrust') 'Delayed Tab changed the username.'
        Assert-True ($passwordBox.Text -eq 'Demo-aTrust!2026') 'Password was lost during the asynchronous Tab transition.'
        Write-Output 'PASS: password survives a 250 ms asynchronous Tab transition.'
        $box.DelayMilliseconds = 900
        $panel = Open-ForReceiver
        Set-Search $panel 'atrust'
        Send-Checked $panel '+{ENTER}'
        for ($attempt=0; $attempt -lt 40 -and $passwordBox.Text -ne 'Demo-aTrust!2026'; $attempt++) { Pump 100 }
        Assert-True ($box.Text -eq 'demo.atrust') 'Slow Tab changed username.'
        Assert-True ($passwordBox.Text -eq 'Demo-aTrust!2026') 'Password was lost during a 900 ms focus transition.'
        Write-Output 'PASS: native input waits for a 900 ms focus transition.'
        $box.DelayMilliseconds = 2400
        $panel = Open-ForReceiver
        Set-Search $panel 'atrust'
        Send-Checked $panel '+{ENTER}'
        $errorText = $null
        for ($attempt=0; $attempt -lt 40; $attempt++) {
            Pump 100
            try { $errorPanel = Find-AppWindow } catch { continue }
            $errorText = $errorPanel.FindFirst([Windows.Automation.TreeScope]::Descendants,
                [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'Tab 未及时切换到下一输入框，已取消密码填充。请点击密码框后单独填入。'))
            if ($null -ne $errorText -and -not $errorText.Current.IsOffscreen) { break }
        }
        $box.CancelTransition()
        Assert-True ($box.Text -eq 'demo.atrust') 'Timeout changed username.'
        Assert-True ($passwordBox.Text -eq '') 'Timed-out Tab unexpectedly sent password.'
        Assert-True ($null -ne $errorText) 'Tab timeout did not report cancellation.'
        [SmokeNative]::SetForegroundWindow([IntPtr]$errorPanel.Current.NativeWindowHandle) | Out-Null
        Pump 150
        Send-Checked ([IntPtr]$errorPanel.Current.NativeWindowHandle) '{ESC}'
        Write-Output 'PASS: native input timeout cancels password without retry.'

    } finally {
        $box.DelayTab = $false
        $box.CancelTransition()
        $box.DelayMilliseconds = 250
    }

    $panel = Open-ForReceiver
    Set-Search $panel 'atrust'
    Send-Checked $panel '+{ENTER}'
    for ($attempt = 0; $attempt -lt 100 -and $box.Text -ne 'demo.atrust'; $attempt++) { Pump 10 }
    Assert-True ($box.Text -eq 'demo.atrust') 'Username was not sent before the focus-change test.'
    $other = [Windows.Forms.Form]::new()
    $other.Text = 'Fake foreground change receiver'
    $otherBox = [Windows.Forms.TextBox]::new()
    $other.Controls.Add($otherBox)
    try {
        $other.Show()
        [Windows.Automation.AutomationElement]::FromHandle($otherBox.Handle).SetFocus()
        Pump 800
        Assert-True ($passwordBox.Text -eq '') 'Password was sent after the target lost foreground.'
        Assert-True ($otherBox.Text -eq '') 'Password reached an unrelated window.'
        Write-Output 'PASS: switching windows during the Tab wait cancels the remaining password.'
    } finally { $other.Dispose() }
    $panel = [IntPtr](Find-AppWindow).Current.NativeWindowHandle
    [SmokeNative]::SetForegroundWindow($panel) | Out-Null
    Pump 150
    Send-Checked $panel '{ESC}'

    $panel = Open-ForReceiver
    Set-Search $panel 'atrust'
    Send-Checked $panel '+{ENTER}'
    for ($attempt=0; $attempt -lt 100 -and $box.Text -ne 'demo.atrust'; $attempt++) { Pump 10 }
    Assert-True ($box.Text -eq 'demo.atrust') 'Username missing before session-lock test.'
    $events = [SmokeNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents')
    [SmokeNative]::PostMessage($events,0x2B1,[IntPtr]7,[IntPtr]::Zero) | Out-Null
    Pump 800
    Assert-True ($passwordBox.Text -eq '') 'Password was sent after session lock.'
    $box.Clear()
    $form.Activate()
    $box.Focus() | Out-Null
    [SmokeNative]::SetForegroundWindow($form.Handle) | Out-Null
    Pump 250
    Send-Checked $form.Handle '^%p'
    Pump 700
    $auth = Find-AppWindow
    $editCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Edit)
    $masterEdit = @($auth.FindAll([Windows.Automation.TreeScope]::Descendants,$editCondition) | Where-Object { -not $_.Current.IsOffscreen })[0]
    $masterEdit.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue('Cross-Backend-Test!2026')
    $unlockCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'解锁')
    $unlock = @($auth.FindAll([Windows.Automation.TreeScope]::Descendants,$unlockCondition) | Where-Object { -not $_.Current.IsOffscreen })[0]
    $unlock.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
    Pump 1600
    Set-Search ([IntPtr]$auth.Current.NativeWindowHandle) 'atrust'
    Send-Checked ([IntPtr]$auth.Current.NativeWindowHandle) '{ENTER}'
    Pump 900
    Assert-True ($box.Text -eq 'Demo-aTrust!2026') 'Popup unlock did not allow password filling.'
    Write-Output 'PASS: session lock cancels pending password; locked popup unlocks and fills the original target.'

    $panel = Open-ForReceiver
    Send-Checked $panel '{ESC}'
    Pump 300
    Assert-True ($box.Text -eq '') 'Escape inserted text.'
    Write-Output 'PASS: cancellation leaves receiver unchanged.'

    $panel = Open-ForReceiver
    Set-Search $panel 'aTrust'
    Send-Checked $panel '{DOWN}'
    Send-Checked $panel '{ENTER}'
    Pump 800
    Assert-True ($box.Text -eq 'Demo-aTrust!2026') 'Arrow selection did not remain within the filtered results.'
    Write-Output 'PASS: keyboard result selection.'

    $panel = Open-ForReceiver
    Set-Search $panel 'no-such-demo-record'
    Send-Checked $panel '{ENTER}'
    Assert-True ($box.Text -eq '') 'Empty results unexpectedly inserted text.'
    Send-Checked $panel '{ESC}'
    Write-Output 'PASS: empty results do not fill.'

    if ($TestSensitive) {
        function Verify-TestPopup([string]$password='Cross-Backend-Test!2026',[int]$wait=1600) {
            $root = Find-AppWindow
            $label = $root.FindFirst([Windows.Automation.TreeScope]::Descendants,
                [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'敏感项验证'))
            Assert-True ($null -ne $label) 'Sensitive fill bypassed verification.'
            Invoke-TestButton 'Keyisle · 快速填充' '密码库主密码'
            $edits = @($root.FindAll([Windows.Automation.TreeScope]::Descendants,
                [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Edit)) | Where-Object { -not $_.Current.IsOffscreen })
            Assert-True ($edits.Count -eq 1) 'Unexpected sensitive prompt input layout.'
            # Selecting master verification already focuses this field; UIA SetFocus can reactivate a hidden popup.
            $edits[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($password)
            Invoke-TestButton 'Keyisle · 快速填充' '验证主密码' 0
            Pump $wait
        }
        $events = [SmokeNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents')
        [SmokeNative]::PostMessage($events,0x8001,[IntPtr]::Zero,[IntPtr]0x202) | Out-Null
        Pump 700
        $manage = Find-AppWindow 'Keyisle · 账号管理'
        $edits = @($manage.FindAll([Windows.Automation.TreeScope]::Descendants,
            [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Edit)) | Where-Object { -not $_.Current.IsOffscreen })
        $edits[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue('atrust')
        Pump 200
        Invoke-TestButton 'Keyisle · 账号管理' '编辑账号'
        $manage = Find-AppWindow 'Keyisle · 账号管理'
        $check = $manage.FindFirst([Windows.Automation.TreeScope]::Descendants,
            [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'敏感项：使用、查看和编辑前再次验证'))
        Assert-True ($null -ne $check) 'Sensitive checkbox missing from editor.'
        $toggle = $check.GetCurrentPattern([Windows.Automation.TogglePattern]::Pattern)
        if ($toggle.Current.ToggleState -ne [Windows.Automation.ToggleState]::On) { $toggle.Toggle() }
        Invoke-TestButton 'Keyisle · 账号管理' '保存账号'
        Invoke-TestButton 'Keyisle · 账号管理' '收起'

        $panel = Open-ForReceiver
        Set-Search $panel 'atrust'
        Send-Checked $panel '{ENTER}'
        Assert-True ($box.Text -eq '' -and $passwordBox.Text -eq '') 'Password sent before verification.'
        Invoke-TestButton 'Keyisle · 快速填充' '取消验证'
        Assert-True ($box.Text -eq '') 'Cancelled verification sent password.'
        Send-Checked $panel '{ESC}'
        Write-Output 'PASS: sensitive fill requires verification; cancelling never sends fields.'

        $panel = Open-ForReceiver
        Set-Search $panel 'atrust'
        Send-Checked $panel '{ENTER}'
        Verify-TestPopup 'wrong-password'
        Assert-True ($box.Text -eq '' -and $passwordBox.Text -eq '') 'Wrong master sent sensitive password.'
        Verify-TestPopup
        Assert-True ($box.Text -eq 'Demo-aTrust!2026' -and $passwordBox.Text -eq '') 'Verified password fill failed.'
        Write-Output 'PASS: wrong re-verification is refused; correct master authorizes only password fill.'

        $panel = Open-ForReceiver
        Set-Search $panel 'atrust'
        Send-Checked $panel '^{ENTER}'
        Assert-True ($box.Text -eq '') 'Ctrl+Enter bypassed sensitive verification.'
        Verify-TestPopup
        Assert-True ($box.Text -eq 'demo.atrust' -and $passwordBox.Text -eq '') 'Verified username fill failed.'
        Write-Output 'PASS: sensitive username fill re-verifies independently.'

        $panel = Open-ForReceiver
        Set-Search $panel 'atrust'
        Send-Checked $panel '+{ENTER}'
        Assert-True ($box.Text -eq '' -and $passwordBox.Text -eq '') 'Combined fill bypassed verification.'
        Verify-TestPopup
        Assert-True ($box.Text -eq 'demo.atrust' -and $passwordBox.Text -eq 'Demo-aTrust!2026') 'Verified paired fill failed.'
        Write-Output 'PASS: sensitive paired fill verifies once for this operation and fills both fields.'

        $panel = Open-ForReceiver
        Set-Search $panel 'atrust'
        Send-Checked $panel '{ENTER}'
        Verify-TestPopup 'Cross-Backend-Test!2026' 0
        $other = [Windows.Forms.Form]::new()
        $other.Text = 'Separate receiver during sensitive verification'
        $otherBox = [Windows.Forms.TextBox]::new(); $other.Controls.Add($otherBox)
        try {
            $other.Show(); $other.Activate(); $otherBox.Focus() | Out-Null
            [SmokeNative]::SetForegroundWindow($other.Handle) | Out-Null
            Pump 1200
            Assert-True ($box.Text -eq '' -and $passwordBox.Text -eq '' -and $otherBox.Text -eq '') 'Verification result filled after foreground switch.'
        } finally { $other.Dispose() }
        [SmokeNative]::SetForegroundWindow($panel) | Out-Null
        Pump 200
        Invoke-TestButton 'Keyisle · 快速填充' '取消验证'
        Send-Checked $panel '{ESC}'
        Write-Output 'PASS: foreground change during re-verification cancels authorization.'

        $panel = Open-ForReceiver
        Set-Search $panel 'atrust'
        Send-Checked $panel '{ENTER}'
        Verify-TestPopup 'Cross-Backend-Test!2026' 0
        [SmokeNative]::PostMessage($events,0x2B1,[IntPtr]7,[IntPtr]::Zero) | Out-Null
        Pump 1200
        Assert-True ($box.Text -eq '' -and $passwordBox.Text -eq '') 'Session lock did not invalidate sensitive verification.'
        $form.Activate(); $box.Focus() | Out-Null
        [SmokeNative]::SetForegroundWindow($form.Handle) | Out-Null
        Pump 200
        Send-Checked $form.Handle '^%p'; Pump 600
        $auth = Find-AppWindow
        $edits = @($auth.FindAll([Windows.Automation.TreeScope]::Descendants,
            [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty,[Windows.Automation.ControlType]::Edit)) | Where-Object { -not $_.Current.IsOffscreen })
        $edits[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue('Cross-Backend-Test!2026')
        Invoke-TestButton 'Keyisle · 快速填充' '解锁' 1600
        $panel = [IntPtr](Find-AppWindow).Current.NativeWindowHandle
        Send-Checked $panel '{ESC}'
        Write-Output 'PASS: session lock invalidates pending sensitive consent without replay.'
    }

    }

    if ($ChromePath) {
        $testPage = Join-Path (Split-Path -Parent $ExePath) 'test-input.html'
        Assert-True (Test-Path -LiteralPath $testPage) 'Local browser test page missing.'
        $profile = Join-Path $DataDir 'chrome-test-profile'
        $pageUri = ([Uri]$testPage).AbsoluteUri
        $chrome = $null
        $chromeWindow = $null
        try {
            $chrome = Start-Process -FilePath $ChromePath -ArgumentList @("--app=$pageUri", "--user-data-dir=$profile", '--no-first-run', '--no-default-browser-check', '--force-renderer-accessibility', '--disable-background-mode') -PassThru
            $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty, $chrome.Id)
            for ($attempt = 0; $attempt -lt 100; $attempt++) {
                Pump 200
                $chromeWindow = [Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children, $condition)
                if ($chromeWindow) { break }
            }
            Assert-True ($null -ne $chromeWindow) 'Isolated Chrome test window did not appear.'
            $condition = [Windows.Automation.AndCondition]::new(
                [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, '可见文本'),
                [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Edit))
            $browserInput = $null
            for ($attempt = 0; $attempt -lt 80; $attempt++) {
                Pump 200
                $browserInput = $chromeWindow.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
                if ($browserInput) { break }
            }
            Assert-True ($null -ne $browserInput) 'Chrome test input not accessible.'
            Click-BrowserInput $chromeWindow $browserInput
            Send-Checked ([IntPtr]$chromeWindow.Current.NativeWindowHandle) '^%p'
            Pump 700
            $panel = [IntPtr](Find-AppWindow).Current.NativeWindowHandle
            Set-Search $panel 'Chrome'
            Send-Checked $panel '{ENTER}'
            Pump 800
            $actual = $browserInput.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value
            Assert-True ($actual -eq 'Demo-Web!2026') "Chrome text input mismatch: $actual"
            Write-Output 'PASS: Chrome isolated local-page input receives the selected fake password.'

            $condition = [Windows.Automation.AndCondition]::new(
                [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, '用户名'),
                [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Edit))
            $usernameInput = $chromeWindow.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
            Assert-True ($null -ne $usernameInput) 'Chrome username input not accessible.'
            Click-BrowserInput $chromeWindow $usernameInput
            Send-Checked ([IntPtr]$chromeWindow.Current.NativeWindowHandle) '^%p'
            Pump 700
            $panel = [IntPtr](Find-AppWindow).Current.NativeWindowHandle
            Set-Search $panel 'Chrome'
            Send-Checked $panel '+{ENTER}'
            $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, '组合填充：通过')
            $combinedStatus = $null
            for ($attempt=0; $attempt -lt 40 -and $null -eq $combinedStatus; $attempt++) {
                Pump 100
                $combinedStatus = $chromeWindow.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
            }
            Assert-True ($null -ne $combinedStatus) 'Chrome combined username/password fill did not pass the local-page check.'
            Write-Output 'PASS: Chrome Shift+Enter fills username and actual password control via Tab.'
        } finally {
            if ($chromeWindow) {
                [SmokeNative]::PostMessage([IntPtr]$chromeWindow.Current.NativeWindowHandle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
                Pump 500
            }
        }
    }

    $panel = Open-ForReceiver
    if ($ScreenshotPath) {
        $element = Find-AppWindow
        $rect = $element.Current.BoundingRectangle
        $bitmap = [Drawing.Bitmap]::new([int]$rect.Width, [int]$rect.Height)
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        try {
            $dc = $graphics.GetHdc()
            try { [SmokeNative]::PrintWindow([IntPtr]$element.Current.NativeWindowHandle, $dc, 2) | Out-Null }
            finally { $graphics.ReleaseHdc($dc) }
            $bitmap.Save($ScreenshotPath, [Drawing.Imaging.ImageFormat]::Png)
        } finally { $graphics.Dispose(); $bitmap.Dispose() }
    }
    Send-Checked $panel '{ESC}'
    Pump 1000
    $realStartupAfter = (Get-ItemProperty -LiteralPath $realRunPath -Name WindowsPasswordManager -ErrorAction SilentlyContinue).WindowsPasswordManager
    Assert-True ($realStartupAfter -eq $realStartupBefore) 'Isolated smoke test changed real startup registration.'
    Write-Output 'PASS: isolated vault launch without a test Run override leaves real startup registration unchanged.'
    $process.Refresh()
    Write-Output ('Idle working set: {0:N1} MiB; private bytes: {1:N1} MiB' -f ($process.WorkingSet64 / 1MB), ($process.PrivateMemorySize64 / 1MB))
} catch {
    Write-Output 'DIAGNOSTICS:'
    if ($process -and $process.HasExited) { Write-Output ('Exit code: ' + $process.ExitCode) }
    if (Test-Path -LiteralPath ($ExePath + '.stderr.log')) { Get-Content -LiteralPath ($ExePath + '.stderr.log') -Encoding UTF8 }
    try {
        $element = Find-AppWindow
        foreach ($node in $element.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)) {
            $value = ''
            $pattern = $null
            if ($node.TryGetCurrentPattern([Windows.Automation.ValuePattern]::Pattern, [ref]$pattern)) { $value = $pattern.Current.Value }
            Write-Output ($node.Current.ControlType.ProgrammaticName + ': ' + $node.Current.Name + ' | ' + $value)
        }
        if ($ScreenshotPath) {
            $rect = $element.Current.BoundingRectangle
            $bitmap = [Drawing.Bitmap]::new([int]$rect.Width, [int]$rect.Height)
            $graphics = [Drawing.Graphics]::FromImage($bitmap)
            try {
                $dc = $graphics.GetHdc()
            try { [SmokeNative]::PrintWindow([IntPtr]$element.Current.NativeWindowHandle, $dc, 2) | Out-Null }
            finally { $graphics.ReleaseHdc($dc) }
                $bitmap.Save($ScreenshotPath, [Drawing.Imaging.ImageFormat]::Png)
            } finally { $graphics.Dispose(); $bitmap.Dispose() }
        }
    } catch { Write-Output $_.Exception.Message }
    throw
} finally {
    $events = [SmokeNative]::FindWindow('PasswordManagerPrototypeEvents', 'PasswordManagerPrototypeEvents')
    if ($process -and -not $process.HasExited -and $events -ne [IntPtr]::Zero) {
        [SmokeNative]::PostMessage($events, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
        Pump 600
    }
    if ($process -and -not $process.HasExited) { $process.Kill() }
    $form.Dispose()
    $env:PASSWORD_MANAGER_DATA_DIR = $previousDataDir
}

