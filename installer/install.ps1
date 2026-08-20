<#
.SYNOPSIS
    Installs or removes Jutsu Audio for the current user.

.DESCRIPTION
    Copies this release directory to a fixed per-user location, puts the
    command-line tool on the user's PATH, and adds a Start Menu shortcut for the
    editor. Nothing is written outside the user's own profile, nothing needs
    administrator rights, and your projects, exports and preset libraries are
    never touched.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\install.ps1 -Uninstall
#>
[CmdletBinding()]
param(
    # Where the application lives. The default is per-user, so installing and
    # upgrading never ask for a password.
    [string] $Destination = (Join-Path $env:LOCALAPPDATA 'Programs\JutsuAudio'),

    # Undo what a previous run installed.
    [switch] $Uninstall
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$editor = 'jutsu-audio.exe'
$shortcutPath = Join-Path ([Environment]::GetFolderPath('Programs')) 'Jutsu Audio.lnk'

# Uninstalling from inside an installation that is not in the default place:
# the directory the script is sitting in is the one meant, not the default.
if ($Uninstall -and
    -not $PSBoundParameters.ContainsKey('Destination') -and
    (Test-Path -LiteralPath (Join-Path $PSScriptRoot $editor))) {
    $Destination = $PSScriptRoot
}

function Update-UserPath {
    <#
        Adds or removes one directory in the user's own PATH, and reports
        whether anything changed. Deliberately not `setx`: that writes back the
        *merged* user and machine PATH, which both truncates at 1024 characters
        and copies machine entries into the user's own.
    #>
    param(
        [Parameter(Mandatory)] [string] $Directory,
        [switch] $Remove
    )

    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($null -eq $current) { $current = '' }

    $wanted = $Directory.TrimEnd('\')
    $entries = @($current -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ne $wanted })
    if (-not $Remove) { $entries += $Directory }

    $updated = $entries -join ';'
    if ($updated -eq $current) { return $false }

    [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
    return $true
}

function Assert-NotRunning {
    <#
        Windows will not delete a running executable, and an upgrade that has
        already deleted half an installation before finding that out is worse
        than one that never started. So this is checked before anything is
        removed, not discovered partway through.

        Every executable in the directory, not only the editor: the
        command-line tool also runs as a long-lived MCP server, and an upgrade
        that checked one of the two would still destroy the installation when it
        met the other.
    #>
    param([Parameter(Mandatory)] [string] $Directory)

    if (-not (Test-Path -LiteralPath $Directory)) { return }
    foreach ($program in Get-ChildItem -LiteralPath $Directory -Filter *.exe -File) {
        try {
            $stream = [System.IO.File]::Open($program.FullName, 'Open', 'ReadWrite', 'None')
            $stream.Close()
        } catch {
            throw "$($program.Name) is in use. Close Jutsu Audio, and anything else using $($program.BaseName) such as an MCP client, then run this again."
        }
    }
}

function Set-StartMenuShortcut {
    param([Parameter(Mandatory)] [string] $Target)

    $shell = New-Object -ComObject WScript.Shell
    try {
        $link = $shell.CreateShortcut($shortcutPath)
        $link.TargetPath = $Target
        $link.WorkingDirectory = Split-Path -Parent $Target
        $link.Description = 'Jutsu Audio'
        $link.Save()
    } finally {
        [void][Runtime.InteropServices.Marshal]::ReleaseComObject($shell)
    }
}

if ($Uninstall) {
    if (Test-Path -LiteralPath $shortcutPath) {
        Remove-Item -LiteralPath $shortcutPath -Force
        Write-Host 'Removed the Start Menu shortcut.'
    }

    if (Update-UserPath -Directory $Destination -Remove) {
        Write-Host "Removed $Destination from your PATH."
    }

    if (Test-Path -LiteralPath (Join-Path $Destination $editor)) {
        Assert-NotRunning -Directory $Destination
        # Step out of the directory first: Windows will not delete the one the
        # running process is sitting in.
        Set-Location -LiteralPath $env:TEMP
        try {
            Remove-Item -LiteralPath $Destination -Recurse -Force
            Write-Host "Deleted $Destination."
        } catch {
            Write-Warning "Could not delete $Destination ($($_.Exception.Message)). Close anything running from it and delete it by hand."
        }
    } else {
        Write-Host "Nothing installed at $Destination."
    }

    Write-Host ''
    Write-Host 'Your projects, exports and preset libraries were not touched.'
    return
}

$source = $PSScriptRoot
if (-not (Test-Path -LiteralPath (Join-Path $source $editor))) {
    throw "$editor is not beside this script. Run install.ps1 from the release directory it came in."
}

if ($source.TrimEnd('\') -ne $Destination.TrimEnd('\')) {
    if (Test-Path -LiteralPath $Destination) {
        # Clearing a directory is only ever safe when this installer wrote it.
        # Anything else is somebody's data, and guessing about that is a bug.
        $existing = @(Get-ChildItem -LiteralPath $Destination -Force)
        $ours = Test-Path -LiteralPath (Join-Path $Destination $editor)
        Assert-NotRunning -Directory $Destination
        if ($existing.Count -gt 0 -and -not $ours) {
            throw "$Destination already exists and is not a Jutsu Audio installation. Pass -Destination <path> to install somewhere else."
        }
        # Piped rather than a `\*` path: a wildcard is not a literal path, and
        # an upgrade that silently keeps the old version's files is not one.
        $existing | Remove-Item -Recurse -Force
    } else {
        $null = New-Item -ItemType Directory -Path $Destination -Force
    }

    Copy-Item -Path (Join-Path $source '*') -Destination $Destination -Recurse -Force
}

Set-StartMenuShortcut -Target (Join-Path $Destination $editor)
$pathChanged = Update-UserPath -Directory $Destination

Write-Host "Jutsu Audio is installed in $Destination."
Write-Host '  Start Menu: search for "Jutsu Audio".'
if ($pathChanged) {
    Write-Host '  PATH: added. Open a new terminal, then run: jutsu-audio-cli --version'
} else {
    Write-Host '  PATH: already there.'
}
Write-Host ''
Write-Host 'Remove it again with:'
Write-Host ('  powershell -ExecutionPolicy Bypass -File "{0}" -Uninstall' -f (Join-Path $Destination 'install.ps1'))
