#!/usr/bin/env python3
"""APFS and real Git/Cargo regression tests; no network dependencies."""

import fcntl
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("worktree-cow.py").resolve()
REPO = SCRIPT.parents[2]
spec = importlib.util.spec_from_file_location("worktree_cow", SCRIPT)
cow = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cow)


@unittest.skipUnless(sys.platform == "darwin", "requires APFS clonefile")
class WorktreeCowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.toolchain = subprocess.check_output(
            ["rustup", "show", "active-toolchain"], cwd=REPO, text=True
        ).split()[0]

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="worktree-cow-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve() / "primary with spaces"
        self.root.mkdir()
        self.env = os.environ.copy()
        for key in ("CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC_WRAPPER", "RUSTFLAGS", "GIT_DIR", "GIT_WORK_TREE"):
            self.env.pop(key, None)
        self.env["RUSTUP_TOOLCHAIN"] = self.toolchain
        self.env["BOBCAT_WORKTREE_COW"] = "0"
        self.command("git", "init", "-q")
        self.command("git", "config", "user.name", "CoW test")
        self.command("git", "config", "user.email", "cow@example.invalid")
        (self.root / "src").mkdir()
        (self.root / "Cargo.toml").write_text(
            '[package]\nname="cow-probe"\nversion="0.1.0"\nedition="2021"\n'
            '[dependencies]\nlocal-value={path="local-value"}\n'
        )
        (self.root / "src/main.rs").write_text('fn main() { println!("{}", local_value::value()); }\n')
        (self.root / "local-value/src").mkdir(parents=True)
        (self.root / "local-value/Cargo.toml").write_text(
            '[package]\nname="local-value"\nversion="0.1.0"\nedition="2021"\n'
        )
        (self.root / "local-value/src/lib.rs").write_text('pub fn value() -> u32 { 1 }\n')
        (self.root / ".gitignore").write_text('/target\n')
        self.command("cargo", "build", "--offline", "-q")
        self.command("git", "add", ".")
        self.command("git", "commit", "-qm", "fixture")
        # The installed hook must work on an older commit without these files.
        (self.root / ".github/scripts").mkdir(parents=True)
        shutil.copyfile(SCRIPT, self.root / ".github/scripts/worktree-cow.py")
        (self.root / ".githooks").mkdir()
        shutil.copyfile(REPO / ".githooks/post-checkout", self.root / ".githooks/post-checkout")
        self.command("python3", str(SCRIPT), "install")

    def command(self, *args, cwd=None, check=True):
        return subprocess.run(args, cwd=cwd or self.root, env=self.env,
                              check=check, text=True, capture_output=True)

    def worktree(self, hook=False):
        dest = self.root.parent / "linked tree"
        self.env["BOBCAT_WORKTREE_COW"] = "1" if hook else "0"
        self.command("git", "worktree", "add", "--detach", str(dest))
        return dest

    def test_hook_seeds_old_branch_and_normal_checkout_preserves_target(self):
        dest = self.worktree(hook=True)
        self.assertTrue((dest / "target" / cow.MARKER).is_file())
        self.assertFalse((dest / ".github/scripts/worktree-cow.py").exists())
        a, b = self.root / "target/debug/cow-probe", dest / "target/debug/cow-probe"
        self.assertEqual(a.read_bytes(), b.read_bytes())
        self.assertNotEqual(a.stat().st_ino, b.stat().st_ino)
        sentinel = dest / "target/keep-me"
        sentinel.write_text("do not replace on checkout")
        self.command("git", "checkout", "--detach", "HEAD", cwd=dest)
        self.assertTrue(sentinel.exists())
        self.command("python3", str(SCRIPT), "seed", "--replace", str(dest))
        self.assertTrue(sentinel.exists())

    def test_old_mtime_path_dependency_rebuilds_after_migration(self):
        dest = self.worktree()
        (dest / "target").mkdir()
        (dest / "target/old-cache").write_text("discard")
        local = dest / "local-value/src/lib.rs"
        local.write_text('pub fn value() -> u32 { 2 }\n')
        os.utime(local, (1, 1))
        before = self.command("git", "status", "--porcelain", cwd=dest).stdout
        self.command("python3", str(SCRIPT), "seed", "--replace", str(dest))
        self.assertFalse((dest / "target/old-cache").exists())
        self.command("cargo", "build", "--offline", "-q", cwd=dest)
        self.assertEqual(self.command(str(dest / "target/debug/cow-probe")).stdout.strip(), "2")
        self.assertEqual(self.command(str(self.root / "target/debug/cow-probe")).stdout.strip(), "1")
        self.assertEqual(local.stat().st_mtime_ns, 1_000_000_000)
        self.assertEqual(before, self.command("git", "status", "--porcelain", cwd=dest).stdout)

    def test_busy_destination_keeps_old_target(self):
        dest = self.worktree()
        profile = dest / "target/debug"
        profile.mkdir(parents=True)
        old = profile / "keep"
        old.write_text("old")
        with (profile / ".cargo-lock").open("wb") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = self.command("python3", str(SCRIPT), "seed", "--replace", str(dest), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(old.exists())
        self.assertIn("Cargo is using", result.stderr)

    def test_clone_failure_keeps_old_target(self):
        dest = self.worktree()
        (dest / "target").mkdir()
        sentinel = dest / "target/keep"
        sentinel.write_text("old")
        with patch.object(cow, "clone_tree", side_effect=OSError("clone unsupported")):
            with self.assertRaises(OSError):
                cow.seed(self.root, dest, set(), True)
        self.assertEqual(sentinel.read_text(), "old")
        self.assertEqual(list(dest.glob(".cow-target-*")), [])

    def test_clone_write_isolation_and_internal_hardlinks(self):
        source = self.root.parent / "data"
        source.mkdir()
        (source / "a").write_bytes(b"original")
        os.link(source / "a", source / "b")
        os.symlink("a", source / "link")
        dest = self.root.parent / "copy"
        cow.clone_tree(source, dest)
        self.assertNotEqual((source / "a").stat().st_ino, (dest / "a").stat().st_ino)
        self.assertEqual((dest / "a").stat().st_ino, (dest / "b").stat().st_ino)
        (dest / "a").write_bytes(b"modified")
        self.assertEqual((source / "a").read_bytes(), b"original")
        self.assertEqual((dest / "b").read_bytes(), b"modified")
        self.assertTrue((dest / "link").is_symlink())

    def test_external_fingerprint_kept_but_build_script_reruns(self):
        profile = self.root / "target/debug"
        entry = profile / ".fingerprint/external-abcdef"
        entry.mkdir()
        (entry / "lib-external").write_text("fingerprint")
        (entry / "run-build-script-build-script-build").write_text("must rerun")
        unknown = profile / ".fingerprint/historical-local-abcdef"
        unknown.mkdir()
        cow.invalidate_local_builds(self.root / "target", {"external"})
        self.assertTrue((entry / "lib-external").exists())
        self.assertFalse((entry / "run-build-script-build-script-build").exists())
        self.assertFalse(unknown.exists())

    def test_seed_omits_local_objects_and_old_incremental_sessions(self):
        profile = self.root / "target/debug"
        (profile / "deps/libexternal_name-123.rlib").write_bytes(b"external")
        (profile / "deps/local_value-old.rcgu.o").write_bytes(b"old local object")
        clone = self.root.parent / "filtered-target"
        cow.clone_tree(self.root / "target", clone, {"external-name"})
        self.assertEqual((clone / "debug/deps/libexternal_name-123.rlib").read_bytes(), b"external")
        self.assertFalse((clone / "debug/deps/local_value-old.rcgu.o").exists())
        self.assertFalse((clone / "debug/incremental").exists())
        self.assertTrue((profile / "deps/local_value-old.rcgu.o").exists())


if __name__ == "__main__":
    unittest.main()
