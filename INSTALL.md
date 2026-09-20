# Installing Araseo

Araseo runs as a native Windows application and opens workspaces from your WSL
distribution. Windows 10 version 1809 or newer, or Windows 11, and WSL2 are
required.

## Install or update from WSL

Run the following command inside WSL:

```sh
curl -fsSL https://github.com/mincheol-lee/araseo/releases/latest/download/install.sh | sh
```

The installer verifies SHA-256 checksums before it changes the installation. It
places the Windows executable in `%LOCALAPPDATA%\Programs\Araseo` and the WSL
launcher in `~/.local/bin/araseo`. If `~/.local/bin` is not already on `PATH`,
the installer prints the required next step.

Open the current directory after installation:

```sh
araseo .
```

Run the same installation command again to update to the newest stable release.
To install a particular version, pass its Git tag:

```sh
curl -fsSL https://github.com/mincheol-lee/araseo/releases/latest/download/install.sh \
  | sh -s -- --version v0.1.8
```

## Manual portable installation

Download `araseo-windows-x86_64.zip` and `SHA256SUMS` from the GitHub Release.
After checking the archive hash, extract it and place `araseo.exe` in a Windows
directory. Install the `araseo` script on the WSL `PATH` and set `ARASEO_EXE`
to the executable's WSL interop path:

```sh
install -m 755 araseo ~/.local/bin/araseo
export ARASEO_EXE=/mnt/c/Tools/Araseo/araseo.exe
araseo .
```

## Uninstall

Run the installer in uninstall mode from WSL:

```sh
curl -fsSL https://github.com/mincheol-lee/araseo/releases/latest/download/install.sh \
  | sh -s -- --uninstall
```

This removes only the installed executable, WSL launcher, and Araseo executable
location file. User preferences under the Windows LocalAppData directory remain
untouched.
