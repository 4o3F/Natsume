# Home templates

The real package installs a versioned immutable `lower/` tree generated from the target OS `/etc/skel` plus managed Browser/IDE defaults. Runtime reset creates a new OverlayFS upper/work pair; it does not recreate the Unix user.
