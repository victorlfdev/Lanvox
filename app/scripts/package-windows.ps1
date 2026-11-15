param(
    [switch]$SkipBuild,
    [switch]$RequireInstaller
)
$ErrorActionPreference = 'Stop'
$projectPath = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$distributionPath = Join-Path $projectPath 'target/distribution'
New-Item -ItemType Directory -Force -Path $distributionPath | Out-Null

if (-not $env:CMAKE) {
    $cmakeCommand = Get-Command cmake -ErrorAction SilentlyContinue
    if ($cmakeCommand) { $env:CMAKE = $cmakeCommand.Source }
    else {
        $vsCmake = Get-ChildItem 'C:/Program Files/Microsoft Visual Studio' -Filter cmake.exe -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($vsCmake) { $env:CMAKE = $vsCmake.FullName }
    }
}

Push-Location $projectPath
try {
    if (-not $SkipBuild) {
        & cargo build --locked --release
        if ($LASTEXITCODE -ne 0) { throw 'Falha ao compilar Lanvox.' }
    }
    $binaryPath = Join-Path $projectPath 'target/release/lanvox.exe'
    if (-not (Test-Path -LiteralPath $binaryPath)) { throw 'Executável release não encontrado.' }
    $manifestText = Get-Content -LiteralPath (Join-Path $projectPath 'Cargo.toml') -Raw
    $appVersion = [regex]::Match($manifestText, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
    if (-not $appVersion) { throw 'Versão ausente no Cargo.toml.' }
    $readmePath = Join-Path $distributionPath 'LEIA-ME.txt'
    @'
Lanvox - conversas privadas em LAN/VPN

Abra lanvox.exe. Crie um host local ou informe o endereço do host dos amigos.
Confirme a impressão digital do host com seu administrador antes da primeira conexão.
Para hospedar sem interface, consulte lanvox.exe --help.

Identidade e preferências ficam na pasta de configuração do usuário, em Lanvox.
Exporte sua identidade nas configurações e guarde o arquivo em local privado.
A desinstalação preserva seus dados e identidade.

Voz e mídia exigem acesso UDP ao host; comandos e mensagens usam TCP.
Use uma VPN configurada ou uma rede local de confiança.
Áudio compartilhado por aplicativo requer Windows com process loopback.
'@ | Set-Content -LiteralPath $readmePath -Encoding utf8
    $noticesPath = Join-Path $distributionPath 'THIRD_PARTY_NOTICES.txt'
    $metadataJson = & cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Falha ao listar as licenças das dependências.' }
    $metadata = $metadataJson | ConvertFrom-Json
    $notices = [System.Collections.Generic.List[string]]::new()
    $notices.Add('Lanvox - third-party dependencies and bundled license notices')
    foreach ($package in ($metadata.packages | Where-Object { $_.source } | Sort-Object name,version)) {
        $notices.Add("`r`n$($package.name) $($package.version) - $($package.license)`r`n$($package.repository)")
        $packageDirectory = Split-Path -Parent $package.manifest_path
        $licenseFiles = Get-ChildItem -LiteralPath $packageDirectory -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)([._-].*)?$' }
        foreach ($licenseFile in $licenseFiles) {
            $notices.Add("`r`n$($licenseFile.Name)`r`n" + (Get-Content -LiteralPath $licenseFile.FullName -Raw))
        }
    }
    # Cisco's binary is downloaded by the app, never bundled; its license must still be shown.
    $notices.Add("`r`nOpenH264 Video Codec provided by Cisco Systems, Inc. (downloaded from Cisco at run time)`r`n" + (Get-Content -LiteralPath (Join-Path $projectPath 'packaging/OPENH264_BINARY_LICENSE.txt') -Raw))
    $notices | Set-Content -LiteralPath $noticesPath -Encoding utf8
    $portablePath = Join-Path $distributionPath "Lanvox-$appVersion-windows-x64.zip"
    Compress-Archive -LiteralPath @($binaryPath, $readmePath, $noticesPath) -DestinationPath $portablePath -Force

    $compilerCandidates = @(
        $env:LANVOX_ISCC,
        (Join-Path $projectPath '../.tools/innosetup/ISCC.exe'),
        'C:/Program Files/Inno Setup 7/ISCC.exe',
        'C:/Program Files (x86)/Inno Setup 6/ISCC.exe',
        'C:/Program Files/Inno Setup 6/ISCC.exe'
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }
    $compiler = $compilerCandidates | Select-Object -First 1
    if ($compiler) {
        & $compiler "/DAppVersion=$appVersion" "/DSourceBinary=$binaryPath" "/DSourceReadme=$readmePath" "/DSourceNotices=$noticesPath" "/DDistributionDir=$distributionPath" (Join-Path $projectPath 'packaging/lanvox.iss')
        if ($LASTEXITCODE -ne 0) { throw 'Falha ao gerar o instalador.' }
    }
    elseif ($RequireInstaller) { throw 'ISCC não encontrado. Instale Inno Setup ou informe LANVOX_ISCC.' }
    else { Write-Warning 'ZIP portátil criado; instalador requer Inno Setup (LANVOX_ISCC).' }

    $artifacts = Get-ChildItem -LiteralPath $distributionPath -File | Where-Object { $_.Name -like "Lanvox-$appVersion-*" }
    $hashLines = $artifacts | ForEach-Object {
        $hash = Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
        "$($hash.Hash.ToLowerInvariant())  $($_.Name)"
    }
    $hashLines | Set-Content -LiteralPath (Join-Path $distributionPath 'SHA256SUMS.txt') -Encoding ascii
    Write-Host "Distribuição criada em $distributionPath"
}
finally { Pop-Location }
