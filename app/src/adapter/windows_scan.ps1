$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$minBytes = [long]__MIN_BYTES__
$maxDepth = [int]__MAX_DEPTH__
$dirs = [System.Collections.Generic.List[object]]::new()
$images = [System.Collections.Generic.List[object]]::new()

function Measure-Dir([System.IO.DirectoryInfo]$dir, [int]$depth) {
    $total = [long]0
    $entries = @()
    try { $entries = $dir.GetFileSystemInfos() } catch { return $total }
    foreach ($entry in $entries) {
        if ($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { continue }
        if ($entry -is [System.IO.DirectoryInfo]) {
            $total += Measure-Dir $entry ($depth + 1)
            continue
        }
        $total += $entry.Length
        if ($entry.Name -eq 'ext4.vhdx' -and $entry.Length -ge $minBytes) {
            $images.Add(@{ path = $entry.FullName; bytes = $entry.Length })
        }
    }
    if ($depth -gt 0 -and $depth -le $maxDepth -and $total -ge $minBytes) {
        $dirs.Add(@{ path = $dir.FullName; bytes = $total })
    }
    return $total
}

$drive = Get-PSDrive C
$walked = Measure-Dir ([System.IO.DirectoryInfo]::new('C:\')) 0
@{
    capacity = [long]($drive.Used + $drive.Free)
    used = [long]$drive.Used
    walked = $walked
    dirs = $dirs.ToArray()
    images = $images.ToArray()
} | ConvertTo-Json -Depth 4 -Compress
