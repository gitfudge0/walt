# Next Steps

This branch implements the upstream changes needed for AUR packaging, but the stable `walt` package is not publishable until the `v0.6.0` release tarball exists.

## 1. Review the branch

Check the current diff and packaging files:

```bash
git status
git diff --stat origin/main...HEAD
sed -n '1,220p' packaging/aur/walt/PKGBUILD
sed -n '1,220p' packaging/aur/walt-git/PKGBUILD
```

## 2. Merge this branch

Open a PR or merge `feat/aur-packaging` into your main branch once you are satisfied with the changes.

## 3. Create and push the stable release tag

The stable AUR package expects the upstream tag `v0.6.0`.

```bash
git checkout main
git pull --ff-only
git tag -a v0.6.0 -m "Release v0.6.0"
git push origin main
git push origin v0.6.0
```

Then verify the tarball exists:

```bash
curl -I https://github.com/gitfudge0/walt/archive/refs/tags/v0.6.0.tar.gz
```

## 4. Replace the temporary stable checksum

Once the tag tarball is live, update the stable package checksum:

```bash
cd packaging/aur/walt
updpkgsums
makepkg --printsrcinfo > .SRCINFO
cd ../../..
```

Commit that checksum update before publishing the stable AUR package.

## 5. Validate the PKGBUILDs on an Arch machine

Run these on an Arch Linux environment with `base-devel` installed:

```bash
cd packaging/aur/walt
makepkg -si
namcap PKGBUILD *.pkg.tar.*

cd ../walt-git
makepkg -si
namcap PKGBUILD *.pkg.tar.*
```

What to check:

- `walt` installs `/usr/bin/walt`
- `walt-git` installs `/usr/bin/walt`
- `walt rotation install` writes a user unit with `ExecStart=/usr/bin/walt --rotate-daemon`
- `walt uninstall --yes` leaves `/usr/bin/walt` in place for packaged installs

## 6. Create the AUR repositories

Create these package bases in AUR:

- `walt`
- `walt-git`

Then clone them:

```bash
git clone ssh://aur@aur.archlinux.org/walt.git /tmp/aur-walt
git clone ssh://aur@aur.archlinux.org/walt-git.git /tmp/aur-walt-git
```

## 7. Publish the package files

Copy the upstream-tracked files into each AUR repo:

```bash
cp packaging/aur/walt/PKGBUILD packaging/aur/walt/.SRCINFO /tmp/aur-walt/
cp packaging/aur/walt-git/PKGBUILD packaging/aur/walt-git/.SRCINFO /tmp/aur-walt-git/
```

Commit and push:

```bash
cd /tmp/aur-walt
git add PKGBUILD .SRCINFO
git commit -m "Initial release"
git push

cd /tmp/aur-walt-git
git add PKGBUILD .SRCINFO
git commit -m "Initial release"
git push
```

## 8. Verify install with an AUR helper

On an Arch machine:

```bash
yay -S walt
yay -S walt-git
```

Also verify removal:

```bash
pacman -R walt
pacman -R walt-git
```

## 9. Ongoing maintenance

For each stable release:

```bash
cd packaging/aur/walt
updpkgsums
makepkg --printsrcinfo > .SRCINFO
```

For `walt-git`, regenerate `.SRCINFO` only when package metadata changes:

```bash
cd packaging/aur/walt-git
makepkg --printsrcinfo > .SRCINFO
```
