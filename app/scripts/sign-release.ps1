<#
.SYNOPSIS
  Assina o manifesto de atualização de uma release rascunho e anexa os arquivos a ela.

.DESCRIPTION
  Roda na máquina do mantenedor (a chave privada nunca vai para o GitHub). Baixa o instalador do
  rascunho vX.Y.Z, gera update-manifest.json e update-manifest.json.sig com a chave privada e
  anexa os dois à release. Com -Publish, também publica o rascunho.

.EXAMPLE
  ./scripts/sign-release.ps1 -Version 0.1.1 -Notes "Correções de compartilhamento de tela"

.EXAMPLE
  $env:LANVOX_UPDATE_KEY = 'C:\privado\lanvox-update.key'
  ./scripts/sign-release.ps1 -Version 0.1.1 -Notes "..." -Publish
#>
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Notes = '',
    [string]$KeyPath = $env:LANVOX_UPDATE_KEY,
    [string]$Repo,
    [switch]$Publish
)
$ErrorActionPreference = 'Stop'

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'Version deve ser x.y.z, só números (ex.: 0.1.1).' }
if (-not $KeyPath) { throw 'Informe -KeyPath ou defina LANVOX_UPDATE_KEY com o caminho da chave privada.' }
if (-not (Test-Path -LiteralPath $KeyPath)) { throw "Chave privada não encontrada: $KeyPath" }
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { throw 'GitHub CLI (gh) não encontrado.' }

$projectPath = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$tag = "v$Version"
$ghRepo = @()
if ($Repo) { $ghRepo = @('--repo', $Repo) }

# A versão do Cargo.toml precisa bater com a tag, igual ao workflow de release.
$cargoText = Get-Content -LiteralPath (Join-Path $projectPath 'Cargo.toml') -Raw
$cargoVersion = [regex]::Match($cargoText, '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if ($cargoVersion -ne $Version) { throw "Cargo.toml está em $cargoVersion, mas você pediu $Version." }

# O rascunho precisa existir e ainda não estar publicado.
$release = gh release view $tag @ghRepo --json isDraft,assets | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "Release $tag não encontrada. O workflow de release já terminou?" }
if (-not $release.isDraft) { throw "A release $tag já foi publicada; não reassine nem republique um número lançado." }
if ($release.assets | Where-Object { $_.name -like 'update-manifest.json*' }) {
    throw "A release $tag já tem manifesto anexado. Remova-o no GitHub antes de assinar de novo."
}

$work = Join-Path ([IO.Path]::GetTempPath()) "lanvox-sign-$Version-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $work | Out-Null
try {
    $downloads = Join-Path $work 'download'
    $signed = Join-Path $work 'assinatura'

    Write-Host "Baixando o instalador de $tag..."
    gh release download $tag @ghRepo --pattern '*-setup.exe' --dir $downloads
    if ($LASTEXITCODE -ne 0) { throw 'Falha ao baixar o instalador.' }
    $installer = Get-ChildItem -LiteralPath $downloads -Filter '*-setup.exe' | Select-Object -First 1
    if (-not $installer) { throw 'Instalador não encontrado na release.' }
    Write-Host "Instalador: $($installer.Name) ($([math]::Round($installer.Length / 1MB, 1)) MB)"

    Write-Host 'Assinando o manifesto...'
    Push-Location $projectPath
    try {
        & cargo run --locked --release -- --update-sign $installer.FullName --update-key $KeyPath --update-version $Version --update-notes $Notes --out $signed
        if ($LASTEXITCODE -ne 0) { throw 'Falha ao assinar o manifesto.' }
    }
    finally { Pop-Location }

    $manifest = Join-Path $signed 'update-manifest.json'
    $signature = Join-Path $signed 'update-manifest.json.sig'
    foreach ($file in @($manifest, $signature)) {
        if (-not (Test-Path -LiteralPath $file)) { throw "Arquivo não gerado: $file" }
    }

    Write-Host 'Anexando manifesto e assinatura ao rascunho...'
    gh release upload $tag $manifest $signature @ghRepo
    if ($LASTEXITCODE -ne 0) { throw 'Falha ao anexar os arquivos.' }

    if ($Publish) {
        Write-Host "Publicando $tag..."
        gh release edit $tag --draft=false @ghRepo
        if ($LASTEXITCODE -ne 0) { throw 'Falha ao publicar a release.' }
        Write-Host "Release $tag publicada."
    }
    else {
        Write-Host "Pronto. Revise o rascunho no GitHub e publique, ou rode de novo com -Publish na próxima vez."
    }
}
finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
