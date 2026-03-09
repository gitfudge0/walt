# Walt AUR Packaging

This directory holds the canonical upstream packaging files for the AUR packages:

- `walt`
- `walt-git`

Publish each package through its own AUR git repository:

- `ssh://aur@aur.archlinux.org/walt.git`
- `ssh://aur@aur.archlinux.org/walt-git.git`

## Update Workflow

1. Copy the contents of `packaging/aur/walt/` into the `walt` AUR repo.
2. Copy the contents of `packaging/aur/walt-git/` into the `walt-git` AUR repo.
3. Commit and push each AUR repo separately.

## Regenerating `.SRCINFO`

From the package directory:

```bash
makepkg --printsrcinfo > .SRCINFO
```

Run this separately in:

- `packaging/aur/walt`
- `packaging/aur/walt-git`

## Stable Package Release Notes

The stable package targets the upstream tag `v0.6.0`.

Until that tag is pushed and GitHub serves the release tarball, the stable `PKGBUILD` uses `sha256sums=('SKIP')` as a temporary placeholder. Replace it with the real checksum before publishing the `walt` AUR package:

```bash
updpkgsums
makepkg --printsrcinfo > .SRCINFO
```
