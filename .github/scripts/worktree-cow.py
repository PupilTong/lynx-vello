#!/usr/bin/env python3
"""Seed independent worktree targets with APFS clones of the primary checkout.

Requires Python 3.9+ and macOS. Stop builds/rust-analyzer before --replace;
Cargo locks detect existing builds, but cannot prevent a new build starting
against a directory while it is being replaced. No file-content comparison,
compilation, dependency installation, or source-file mutation happens here.
"""

from __future__ import annotations

import argparse
from contextlib import ExitStack, contextmanager
import ctypes
import fcntl
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


MARKER = ".bobcat-cow.json"
HOOK_MARKER = "# lynx-vello worktree CoW hook"


def run(*args: str, cwd: Path) -> str:
    # Git exports these to hooks; they otherwise override `git -C`/cwd when
    # inspecting another worktree. Preserve unrelated environment settings.
    env = os.environ.copy()
    for key in (
        "GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX", "GIT_IMPLICIT_WORK_TREE",
    ):
        env.pop(key, None)
    result = subprocess.run(args, cwd=cwd, env=env, text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "command failed: " + " ".join(args))
    return result.stdout.strip()


def common_dir(root: Path) -> Path:
    return Path(run("git", "rev-parse", "--path-format=absolute", "--git-common-dir", cwd=root))


def primary_checkout(root: Path) -> Path:
    common = common_dir(root)
    if common.name != ".git":
        raise RuntimeError("requires a primary checkout with a .git directory")
    return common.parent


def profiles(target: Path) -> list[Path]:
    # Native and cross-compilation layouts, including custom profiles. Avoid
    # recursively traversing the hundreds of thousands of files in deps/.
    return sorted({p.parent for pattern in ("*/.cargo-lock", "*/*/.cargo-lock")
                   for p in target.glob(pattern) if not p.parent.is_symlink()})


@contextmanager
def cargo_locks(target: Path):
    with ExitStack() as stack:
        for profile in profiles(target):
            handle = stack.enter_context((profile / ".cargo-lock").open("rb"))
            try:
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise RuntimeError(f"Cargo is using {profile}; stop that build and retry") from None
        yield


def external_packages(source: Path) -> set[str]:
    metadata = json.loads(run("cargo", "metadata", "--offline", "--locked",
                              "--format-version", "1", cwd=source))
    if Path(metadata["target_directory"]).resolve() != source / "target":
        raise RuntimeError("custom Cargo target directories are not supported")
    if Path(metadata.get("build_directory", metadata["target_directory"])).resolve() != source / "target":
        raise RuntimeError("custom Cargo build directories are not supported")
    local = {p["name"] for p in metadata["packages"] if p["source"] is None}
    return {p["name"] for p in metadata["packages"] if p["source"] is not None} - local


def clone_tree(source: Path, destination: Path, external: set[str] | None = None) -> None:
    # Unlike cp -c, clonefile cannot silently fall back to a full data copy.
    clone = ctypes.CDLL(None, use_errno=True).clonefile
    clone.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_int]
    clone.restype = ctypes.c_int
    links: dict[tuple[int, int], str] = {}
    profile_paths = set(profiles(source)) if external is not None else set()
    artifact_names = {name.replace("-", "_") for name in (external or set())}

    def ignore(directory: str, names: list[str]) -> set[str]:
        path = Path(directory)
        if path in profile_paths:
            # Local packages must rebuild. Copying their accumulated incremental
            # sessions and object files only multiplies obsolete file metadata.
            return {"incremental", "examples"} & set(names)
        if path.parent in profile_paths and path.name == "deps":
            def reusable(name: str) -> bool:
                crate = name.split("-", 1)[0]
                return crate in artifact_names or (
                    crate.startswith("lib") and crate[3:] in artifact_names
                )
            return {name for name in names if not reusable(name)}
        return set()

    def copy_file(src: str, dst: str) -> str:
        st = os.stat(src, follow_symlinks=False)
        key = (st.st_dev, st.st_ino)
        if st.st_nlink > 1 and key in links:
            # Preserve Cargo/rustc's internal hard links, never link back into
            # the source checkout. Each worktree still owns independent inodes.
            os.link(links[key], dst)
        else:
            if clone(os.fsencode(src), os.fsencode(dst), 1):  # CLONE_NOFOLLOW
                errno = ctypes.get_errno()
                raise OSError(errno, os.strerror(errno), src)
            if st.st_nlink > 1:
                links[key] = dst
        return dst

    shutil.copytree(source, destination, symlinks=True, copy_function=copy_file, ignore=ignore)


def invalidate_local_builds(target: Path, external: set[str]) -> None:
    for profile in profiles(target):
        fingerprint = profile / ".fingerprint"
        if not fingerprint.is_dir() or fingerprint.is_symlink():
            continue
        for entry in fingerprint.iterdir():
            name = entry.name.rsplit("-", 1)[0]
            if not entry.is_dir() or entry.is_symlink():
                continue
            if name not in external:
                # Also invalidate historical packages no longer in metadata.
                # Old source mtimes must not make another branch look fresh.
                shutil.rmtree(entry)
            else:
                # Even external build scripts emit absolute OUT_DIR paths.
                # Keep their compiled executables, but rerun the scripts.
                for stamp in entry.glob("run-build-script-*"):
                    stamp.unlink()
        build = profile / "build"
        if build.is_dir() and not build.is_symlink():
            for entry in build.iterdir():
                if entry.is_dir() and not entry.is_symlink():
                    if entry.name.rsplit("-", 1)[0] not in external:
                        shutil.rmtree(entry)


def seed(source: Path, root: Path, external: set[str], replace: bool) -> bool:
    if root == source:
        return False
    if common_dir(root) != common_dir(source):
        raise RuntimeError(f"not a worktree of {source}: {root}")
    if Path(run("git", "rev-parse", "--show-toplevel", cwd=root)).resolve() != root:
        raise RuntimeError(f"not a worktree root: {root}")
    target = root / "target"
    if target.is_symlink():
        raise RuntimeError(f"refusing to replace a symlink: {target}")
    if (target / MARKER).exists():
        print(f"already seeded: {root}", flush=True)
        return False
    if target.exists() and not replace:
        print(f"keeping existing target: {root} (use seed --replace to migrate)", flush=True)
        return False
    if not target.exists() and replace:
        # A batch migration need not create caches for inactive/documentation
        # worktrees that have never built anything.
        print(f"no target to migrate: {root}", flush=True)
        return False
    print(f"cloning target into {root}", flush=True)
    with cargo_locks(target), tempfile.TemporaryDirectory(prefix=".cow-target-", dir=root) as temp:
        staging = Path(temp) / "target"
        clone_tree(source / "target", staging, external)
        invalidate_local_builds(staging, external)
        (staging / MARKER).write_text(json.dumps({"source": str(source), "version": 1}) + "\n")
        old = Path(temp) / "previous-target"
        if target.exists():
            target.rename(old)
        try:
            staging.rename(target)
        except BaseException:
            if old.exists():
                old.rename(target)
            raise
        # The previous target is removed only after cloning and invalidation
        # succeed. TemporaryDirectory also removes a failed partial clone.
    print(f"seeded: {root}; next Cargo build rebuilds local packages", flush=True)
    return True


def install(source: Path) -> None:
    configured = subprocess.run(["git", "config", "--get", "core.hooksPath"],
                                cwd=source, text=True, capture_output=True)
    if configured.returncode == 0:
        raise RuntimeError("core.hooksPath is configured; integrate .githooks/post-checkout there")
    hook = common_dir(source) / "hooks" / "post-checkout"
    if hook.exists() and HOOK_MARKER not in hook.read_text():
        raise RuntimeError(f"existing hook must be integrated rather than overwritten: {hook}")
    hook.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source / ".githooks" / "post-checkout", hook)
    hook.chmod(0o755)
    print(f"installed {hook}")


def worktrees(source: Path) -> list[Path]:
    records = run("git", "worktree", "list", "--porcelain", "-z", cwd=source).split("\0\0")
    result = []
    for record in records:
        fields = record.split("\0")
        if any(f.startswith("prunable") or f == "bare" for f in fields):
            continue
        for field in fields:
            if field.startswith("worktree "):
                root = Path(field[len("worktree "):]).resolve()
                if root.is_dir() and root != source:
                    result.append(root)
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("install", help="install the shared post-checkout hook")
    command = commands.add_parser("seed", help="clone the primary checkout's target")
    command.add_argument("--replace", action="store_true", help="migrate existing targets; builds must be stopped")
    command.add_argument("--all", action="store_true", help="migrate all registered worktrees (requires --replace)")
    command.add_argument("worktree", nargs="*", type=Path)
    hook = commands.add_parser("hook")
    hook.add_argument("old")
    hook.add_argument("new")
    hook.add_argument("branch")
    args = parser.parse_args()
    automatic = args.command == "hook"
    if automatic and (args.branch != "1" or not args.old or set(args.old) != {"0"}):
        return 0
    if sys.platform != "darwin":
        if automatic:
            return 0
        parser.error("APFS worktree seeding requires macOS")
    if os.environ.get("BOBCAT_WORKTREE_COW") == "0" and automatic:
        return 0
    try:
        cwd = Path.cwd().resolve()
        source = primary_checkout(cwd)
        if args.command == "install":
            install(source)
            return 0
        if automatic:
            if cwd == source or (cwd / "target").exists():
                return 0
            roots, replace = [cwd], False
        else:
            if args.all and (not args.replace or args.worktree):
                parser.error("--all requires --replace and no worktree arguments")
            roots = worktrees(source) if args.all else [p.resolve() for p in (args.worktree or [cwd])]
            replace = args.replace
        if not (source / "target").is_dir() or (source / "target").is_symlink():
            raise RuntimeError("primary checkout has no regular target directory to clone")
        if os.environ.get("CARGO_TARGET_DIR") or os.environ.get("CARGO_BUILD_BUILD_DIR"):
            raise RuntimeError("unset custom Cargo target/build directory overrides before seeding")
        external = external_packages(source)
        failed = False
        with cargo_locks(source / "target"):
            for root in roots:
                try:
                    seed(source, root, external, replace)
                except (OSError, RuntimeError, shutil.Error) as error:
                    print(f"worktree CoW: {root}: {error}", file=sys.stderr, flush=True)
                    failed = True
        return int(failed and not automatic)
    except (OSError, RuntimeError, shutil.Error) as error:
        print(f"worktree CoW: {error}", file=sys.stderr)
        # Checkout has already succeeded. A missing/busy seed must not turn a
        # successful `git worktree add` into an apparent Git failure.
        return 0 if automatic else 1


if __name__ == "__main__":
    sys.exit(main())
