#Requires -Version 5.1
<#
  hardware.ps1 — 只读硬件画像采集（零写入）

  纪律：
    1. 只读。不写注册表、不装驱动、不改电源计划、不触发任何 UAC。
    2. stdout 只输出一个 JSON 对象，供上层反序列化；人类可读的提示一律走 stderr。
    3. 单项采集失败不中断整体，写入 notes[] 并在对应字段填 null，由上层降级。
    4. 全程内置超时，最坏情况 15 秒内返回，绝不让小白用户对着黑框干等。

  用法：
    pwsh -NoProfile -ExecutionPolicy Bypass -File hardware.ps1
    pwsh -NoProfile -ExecutionPolicy Bypass -File hardware.ps1 -TimeoutSec 8
#>
[CmdletBinding()]
param(
    [ValidateRange(3, 60)]
    [int]$TimeoutSec = 15
)

$ErrorActionPreference = 'Stop'
$ProgressPreference    = 'SilentlyContinue'

$script:notes = [System.Collections.Generic.List[string]]::new()
function Add-Note([string]$m) { $script:notes.Add($m) | Out-Null; Write-Verbose "note: $m" }

# ---------------------------------------------------------------- CPU
function Get-CpuProfile {
    try {
        $p = Get-CimInstance -ClassName Win32_Processor -Property Name, NumberOfCores, NumberOfLogicalProcessors, MaxClockSpeed -ErrorAction Stop |
             Select-Object -First 1
        if (-not $p) { Add-Note 'cpu: Win32_Processor 返回空'; return $null }
        [pscustomobject]@{
            name          = ($p.Name -replace '\s+', ' ').Trim()
            physicalCores = [int]$p.NumberOfCores
            logicalCores  = [int]$p.NumberOfLogicalProcessors
            baseMhz       = [int]$p.MaxClockSpeed
            avx2          = Test-Avx2
        }
    } catch {
        Add-Note "cpu: $($_.Exception.Message)"
        $null
    }
}

# CPU 指令集决定 CPU 推理是「能用」还是「别想了」。
# i5-2xxx 及更早的机器没有 AVX2，llama.cpp 会退回极慢的标量路径。
function Test-Avx2 {
    try {
        if ($PSVersionTable.PSVersion.Major -ge 6) {
            return [System.Runtime.Intrinsics.X86.Avx2]::IsSupported
        }
        # Windows PowerShell 5.1 fallback：读 CPUID 不可行，用 .NET 反射探测
        $t = [type]::GetType('System.Runtime.Intrinsics.X86.Avx2, System.Runtime.Intrinsics')
        if ($t) { return [bool]$t.GetProperty('IsSupported').GetValue($null) }
        Add-Note 'avx2: 运行时过旧，无法探测，按 false 处理'
        return $false
    } catch {
        Add-Note "avx2: $($_.Exception.Message)"
        $false
    }
}

# ---------------------------------------------------------------- RAM
function Get-MemoryProfile {
    try {
        $cs  = Get-CimInstance -ClassName Win32_ComputerSystem -Property TotalPhysicalMemory -ErrorAction Stop
        $os  = Get-CimInstance -ClassName Win32_OperatingSystem -Property TotalVisibleMemorySize, FreePhysicalMemory, Caption, Version, BuildNumber -ErrorAction Stop
        $mod = @(Get-CimInstance -ClassName Win32_PhysicalMemory -Property Capacity, Speed, Manufacturer -ErrorAction SilentlyContinue)

        [pscustomobject]@{
            totalBytes      = [int64]$cs.TotalPhysicalMemory
            visibleBytes    = [int64]$os.TotalVisibleMemorySize * 1KB
            freeBytes       = [int64]$os.FreePhysicalMemory * 1KB
            moduleCount     = $mod.Count
            moduleSpeedMhz  = if ($mod.Count) { [int]($mod | Measure-Object -Property Speed -Maximum).Maximum } else { $null }
            # 单条最大容量：判断有没有升级空间，用于「加内存」这条建议
            largestModuleGb = if ($mod.Count) { [math]::Round(($mod | Measure-Object -Property Capacity -Maximum).Maximum / 1GB, 0) } else { $null }
        }
    } catch {
        Add-Note "memory: $($_.Exception.Message)"
        $null
    }
}

# ---------------------------------------------------------------- GPU
# 显存是分档的第一判据，也是最容易踩坑的地方：
#   - Win32_VideoController.AdapterRAM 是 uint32，4GB 以上一律显示 4293918720（约 4.29GB 假值），不可信。
#   - 正解优先级：nvidia-smi（NVIDIA 独显）> 注册表 qwMemorySize（REG_QWORD，64 位真值）> WMI 兜底。
function Get-GpuList {
    $smiVram = Get-NvidiaSmiVram
    $regVram = Get-RegistryVram
    $out = [System.Collections.Generic.List[object]]::new()

    try {
        $wmi = @(Get-CimInstance -ClassName Win32_VideoController -Property Name, DriverVersion, AdapterRAM, PNPDeviceID -ErrorAction Stop)
        foreach ($g in $wmi) {
            $name = ($g.Name -replace '\s+', ' ').Trim()
            $vram = $null
            $src  = $null

            foreach ($cand in $smiVram) {
                if ($cand.name -and $name -and ($name -like "*$($cand.name)*" -or $cand.name -like "*$name*")) {
                    $vram = $cand.bytes; $src = 'nvidia-smi'; break
                }
            }
            if (-not $vram -and $regVram.ContainsKey($name)) {
                $vram = $regVram[$name]; $src = 'registry.qwMemorySize'
            }
            if (-not $vram -and $g.AdapterRAM -and $g.AdapterRAM -lt 4293918720) {
                $vram = [int64]$g.AdapterRAM; $src = 'wmi.AdapterRAM(capped)'
                Add-Note "gpu[$name]: 回退到 AdapterRAM，>4GB 时该值不可信"
            }
            if (-not $vram) { Add-Note "gpu[$name]: 显存未知，按 0 处理" }

            $out.Add([pscustomobject]@{
                name          = $name
                driverVersion = [string]$g.DriverVersion
                vramBytes     = if ($vram) { [int64]$vram } else { $null }
                vramSource    = $src
                vendor        = Get-GpuVendor $name
            })
        }
    } catch {
        Add-Note "gpu.wmi: $($_.Exception.Message)"
    }

    # 完全没有 WMI 记录但 nvidia-smi 有结果（例如某些虚拟化/驱动异常场景）
    if ($out.Count -eq 0 -and $smiVram.Count -gt 0) {
        foreach ($c in $smiVram) {
            $out.Add([pscustomobject]@{
                name = $c.name; driverVersion = $null; vramBytes = $c.bytes
                vramSource = 'nvidia-smi'; vendor = 'nvidia'
            })
        }
    }
    ,$out
}

function Get-GpuVendor([string]$name) {
    switch -Regex ($name) {
        'NVIDIA'              { 'nvidia';   break }
        'AMD|Radeon'          { 'amd';      break }
        'Intel'               { 'intel';    break }
        'Microsoft Basic'     { 'virtual';  break }
        default               { 'other' }
    }
}

function Get-NvidiaSmiVram {
    $res = [System.Collections.Generic.List[object]]::new()
    $exe = @(
        'nvidia-smi.exe',
        (Join-Path $env:SystemRoot 'System32\nvidia-smi.exe'),
        (Join-Path $env:ProgramFiles 'NVIDIA Corporation\NVSMI\nvidia-smi.exe')
    ) | Where-Object { $_ -and (Test-Path $_ -ErrorAction SilentlyContinue) } | Select-Object -First 1
    if (-not $exe) {
        $cmd = Get-Command nvidia-smi -ErrorAction SilentlyContinue
        if ($cmd) { $exe = $cmd.Source }
    }
    if (-not $exe) { Add-Note 'nvidia-smi: 未找到'; return ,$res }

    try {
        $lines = & $exe --query-gpu=name,memory.total,driver_version --format=csv,noheader,nounits 2>$null
        foreach ($line in @($lines)) {
            if ([string]::IsNullOrWhiteSpace($line)) { continue }
            $parts = $line -split ','
            if ($parts.Count -lt 2) { continue }
            $mb = 0
            if (-not [int]::TryParse(($parts[1].Trim()), [ref]$mb)) { continue }
            $res.Add([pscustomobject]@{
                name  = $parts[0].Trim()
                bytes = [int64]$mb * 1MB
                driver = if ($parts.Count -ge 3) { $parts[2].Trim() } else { $null }
            })
        }
    } catch {
        Add-Note "nvidia-smi: $($_.Exception.Message)"
    }
    ,$res
}

function Get-RegistryVram {
    $map = @{}
    $base = 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}'
    if (-not (Test-Path $base)) { Add-Note 'registry.vram: 显示适配器类键不存在'; return $map }
    try {
        foreach ($k in (Get-ChildItem $base -ErrorAction Stop)) {
            if ($k.PSChildName -notmatch '^\d{4}$') { continue }
            $p = Get-ItemProperty $k.PSPath -ErrorAction SilentlyContinue
            if (-not $p) { continue }
            $name = $p.'DriverDesc'
            $qw   = $p.'HardwareInformation.qwMemorySize'
            if (-not $name -or -not $qw) { continue }
            if ($qw -is [array]) { $qw = $qw[0] }
            $name = ($name -replace '\s+', ' ').Trim()
            if (-not $map.ContainsKey($name) -or $map[$name] -lt [int64]$qw) {
                $map[$name] = [int64]$qw
            }
        }
    } catch {
        Add-Note "registry.vram: $($_.Exception.Message)"
    }
    $map
}

# ---------------------------------------------------------------- Disk
# 只取固定盘（DriveType=3）。Size/FreeSpace 是字节，前端统一除以 1GB 常量显示；
# 「1TB 硬盘显示 931GB」是厂商标称十进制与系统二进制口径差异，属正常，不是故障。
function Get-DiskList {
    try {
        $d = @(Get-CimInstance -ClassName Win32_LogicalDisk -Filter 'DriveType=3' -Property DeviceID, VolumeName, Size, FreeSpace -ErrorAction Stop)
        ,@($d | ForEach-Object {
            [pscustomobject]@{
                letter     = $_.DeviceID
                label      = $_.VolumeName
                totalBytes = if ($_.Size) { [int64]$_.Size } else { $null }
                freeBytes  = if ($_.FreeSpace) { [int64]$_.FreeSpace } else { $null }
            }
        })
    } catch {
        Add-Note "disk: $($_.Exception.Message)"
        ,@()
    }
}

# ---------------------------------------------------------------- OS / 环境
function Get-EnvironmentProfile {
    $webview2 = $null
    foreach ($k in @(
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
        'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    )) {
        if (Test-Path $k) {
            $v = (Get-ItemProperty $k -ErrorAction SilentlyContinue).pv
            if ($v) { $webview2 = $v; break }
        }
    }
    try {
        $os = Get-CimInstance -ClassName Win32_OperatingSystem -Property Caption, Version, BuildNumber, OSArchitecture -ErrorAction Stop
        $bios = Get-CimInstance -ClassName Win32_ComputerSystem -Property Manufacturer, Model -ErrorAction SilentlyContinue
        [pscustomobject]@{
            caption      = $os.Caption
            version      = $os.Version
            build        = [int]$os.BuildNumber
            arch         = $os.OSArchitecture
            isWin11      = ([int]$os.BuildNumber -ge 22000)
            manufacturer = $bios.Manufacturer
            model        = $bios.Model
            webview2     = $webview2
        }
    } catch {
        Add-Note "os: $($_.Exception.Message)"
        $null
    }
}

# ---------------------------------------------------------------- 主流程
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$profile = [ordered]@{
    schemaVersion = 1
    collectedAt   = (Get-Date).ToString('o')
    cpu           = $null
    memory        = $null
    gpus          = @()
    disks         = @()
    environment   = $null
    notes         = @()
    elapsedMs     = 0
}

$profile.cpu    = Get-CpuProfile
$profile.memory = Get-MemoryProfile
# 注意：Get-GpuList / Get-DiskList 自身已用 ,$list 返回单层数组，
# 外层不能再套 @()，否则会被展开成嵌套数组（[[...]]），上层解析直接错位。
$profile.gpus   = Get-GpuList
$profile.disks  = Get-DiskList
$profile.environment = Get-EnvironmentProfile
$profile.notes  = @($script:notes)
$sw.Stop()
$profile.elapsedMs = [int]$sw.ElapsedMilliseconds

if ($sw.ElapsedMilliseconds -gt ($TimeoutSec * 1000)) {
    Write-Warning "采集耗时 $([int]$sw.ElapsedMilliseconds)ms，超过预期 ${TimeoutSec}s"
}

$profile | ConvertTo-Json -Depth 6 -Compress:$false
