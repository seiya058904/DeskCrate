<#
.SYNOPSIS
  Checks DeskCrate against its performance budgets.
.DESCRIPTION
  - Release executable size  < 5 MB
  - Idle private memory      < 20 MB   (sampled after the idle trim)
  - Idle CPU                 ~ 0       (CPU time consumed while idle)
  By default it measures an already running DeskCrate. With -Launch it starts
  target\release\deskcrate.exe, measures, then asks it to exit (WM_CLOSE) and
  verifies that no process is left behind.
.EXAMPLE
  pwsh scripts/check-budget.ps1 -Build -Launch
#>
param(
    [switch]$Build,
    [switch]$Launch,
    [int]$IdleSeconds = 20
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\deskcrate.exe'
$fail = $false

if ($Build) {
    Push-Location $root
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    Pop-Location
}

$size = (Get-Item $exe).Length
$ok = $size -lt 5MB
if (-not $ok) { $fail = $true }
'{0}  exe size      {1:N0} KB  (budget 5 MB)' -f ($(if ($ok) { 'PASS' } else { 'FAIL' })), ($size / 1KB)

if ($Launch) {
    if (Get-Process deskcrate -ErrorAction SilentlyContinue) { throw 'DeskCrate is already running; omit -Launch to measure it.' }
    Start-Process $exe | Out-Null
    Start-Sleep -Seconds 8
}
$p = Get-Process deskcrate -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $p) { 'SKIP  runtime checks (DeskCrate not running; use -Launch)'; if ($fail) { exit 1 } else { exit 0 } }

$cpu0 = $p.TotalProcessorTime.TotalMilliseconds
Start-Sleep -Seconds $IdleSeconds
$p.Refresh()
$cpuMs = $p.TotalProcessorTime.TotalMilliseconds - $cpu0
$private = $p.PrivateMemorySize64
$ok = $private -lt 20MB
if (-not $ok) { $fail = $true }
'{0}  idle private  {1:N1} MB  (budget 20 MB; working set {2:N1} MB, {3} threads)' -f ($(if ($ok) { 'PASS' } else { 'FAIL' })), ($private / 1MB), ($p.WorkingSet64 / 1MB), $p.Threads.Count
$ok = $cpuMs -lt (5 * $IdleSeconds)   # < 0.5% of one core
if (-not $ok) { $fail = $true }
'{0}  idle CPU      {1:N1} ms over {2} s' -f ($(if ($ok) { 'PASS' } else { 'FAIL' })), $cpuMs, $IdleSeconds

if ($Launch) {
    Add-Type -Namespace DC -Name W -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern System.IntPtr FindWindowW(string c, System.IntPtr n);
[DllImport("user32.dll")] public static extern bool PostMessageW(System.IntPtr h, uint m, System.IntPtr w, System.IntPtr l);
'@
    $h = [DC.W]::FindWindowW('DeskCrateHost', [IntPtr]::Zero)
    [DC.W]::PostMessageW($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    Start-Sleep -Seconds 2
    $left = Get-Process deskcrate -ErrorAction SilentlyContinue
    $ok = -not $left
    if (-not $ok) { $fail = $true }
    '{0}  clean exit    {1}' -f ($(if ($ok) { 'PASS' } else { 'FAIL' })), ($(if ($ok) { 'no process left' } else { 'process still running' }))
}
if ($fail) { exit 1 }
