param(
    [string]$Pentool = ".\target\release\pentool.exe",
    [string]$Baseline = "docs\render-benchmark-v0.6.1-100000-paths.json",
    [double]$MaxRegressionPercent = 15
)
$ErrorActionPreference = "Stop"
$expected = Get-Content -Raw -LiteralPath $Baseline | ConvertFrom-Json
$currentPath = Join-Path ([System.IO.Path]::GetTempPath()) "pentool-render-current-$PID.json"
try {
    & $Pentool benchmark --render --layers 1000 --objects 100000 --paths-only --warmups 2 --repetitions 7 --scale 1 --json | Set-Content -Encoding utf8 -LiteralPath $currentPath
    if ($LASTEXITCODE -ne 0) { throw "renderer benchmark failed" }
    $actual = Get-Content -Raw -LiteralPath $currentPath | ConvertFrom-Json
    $limit = 1 + $MaxRegressionPercent / 100
    foreach ($metric in @("total", "svg_construction", "usvg_parse", "rasterization", "png_encoding")) {
        $before = [double]$expected.timings_us.$metric.median
        $after = [double]$actual.timings_us.$metric.median
        if ($after -gt $before * $limit) { throw "$metric regressed: $after us versus $before us baseline" }
    }
    if ($actual.render.output_bytes -gt $expected.render.output_bytes * 1.10) { throw "PNG artifact size regressed by more than 10%" }
    if ($null -ne $expected.peak_resident_bytes -and $null -ne $actual.peak_resident_bytes -and $actual.peak_resident_bytes -gt $expected.peak_resident_bytes * $limit) { throw "peak resident memory regressed" }
    Copy-Item -LiteralPath $currentPath -Destination "render-performance-current.json" -Force
} finally { Remove-Item -LiteralPath $currentPath -Force -ErrorAction SilentlyContinue }
