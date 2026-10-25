# Home templates

The image supplies a versioned immutable template from its final `/etc/skel` and managed Browser/IDE defaults. The Client package supplies validation and the fixed-path reset mechanism; it does not create accounts or templates.

Required layout below this directory:

```
sha256-<64 lowercase hex>/home.squashfs
sha256-<64 lowercase hex>/SHA256SUMS
sha256-<64 lowercase hex>/packages.tsv
current/version
current/lower/
```

`current/version` contains the selected `sha256-...` directory name, optionally followed by one newline. The image builder/installer must verify the image's SHA-256 and archive its provenance. The base, current and version directories, version file and image are root-owned, not group/world writable, and not symlinks.

Mount that exact image, read-only as SquashFS through a loop device, at the real `current/lower` directory. No nested or stacked mounts are allowed there. Keep normal owner write bits inside the image and assign the fixed teams UID/GID, so OverlayFS copy-up produces editable files. Home and the lower root must belong to the actual fixed teams account (`/home/teams`).

Helper checks the selected version, root-controlled source, real mount options, kernel loop backing path and ownership during Home operations and contest login. It does not hash the entire image on every observation tick; content verification belongs to the trusted image build/install boundary.

The mount unit must precede Helper's Home processing without making a failed template mount stop GDM/waiting. Use a fixed mount unit and a Helper Wants/After drop-in, not a global display-manager gate. Enable services against the image root during construction; do not start them in the build chroot.

Update the selected source only during offline, drained Home maintenance, after normal unmount of the old Home. Do not replace lower under a running contest, erase maintenance windows, or invent completion epochs to install a template.

The image project owns template creation, provenance and target-machine validation. Required Client environment conditions are documented in the repository's [Client environment guide](https://github.com/4o3F/Natsume/blob/v2/docs/client-environment.zh-CN.md). The `contest` role token stays unchanged when its Unix account is named `teams`.
