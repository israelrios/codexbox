import importlib.util
import sys
import tempfile
import unittest
from importlib.machinery import SourceFileLoader
from pathlib import Path
from typing import Protocol, cast

import codexbox_image_sync as image_sync


class HostPodmanStoreLike(Protocol):
    image_store: str


class CodexboxModule(Protocol):
    def host_podman_store(
        self,
        podman_info: dict[str, object],
    ) -> HostPodmanStoreLike | None: ...

    def host_podman_additional_image_store(
        self,
        podman_info: dict[str, object],
        home_dir: Path,
        use_fuse_overlayfs: bool,
    ) -> str | None: ...

    def load_ignore_patterns(self, ignore_path: Path) -> list[str]: ...

    def is_ignored_env_var(self, name: str, patterns: list[str]) -> bool: ...


REPO_ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = REPO_ROOT / "codexbox"
LOADER = SourceFileLoader("codexbox_script", str(MODULE_PATH))
SPEC = importlib.util.spec_from_loader(LOADER.name, LOADER)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"Unable to load module from {MODULE_PATH}")
module = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = module
SPEC.loader.exec_module(module)
CODEXBOX = cast(CodexboxModule, cast(object, module))


class HostPodmanStoreTests(unittest.TestCase):
    def test_prefers_driver_specific_imagestore_over_graph_root(self) -> None:
        store = CODEXBOX.host_podman_store(
            {
                "store": {
                    "graphDriverName": "overlay",
                    "graphRoot": "/var/lib/containers/storage",
                    "graphOptions": {
                        "overlay.imagestore": "/home/test/.local/share/containers/storage",
                    },
                }
            }
        )

        self.assertIsNotNone(store)
        assert store is not None
        self.assertEqual(store.image_store, "/home/test/.local/share/containers/storage")

    def test_falls_back_to_generic_imagestore_key(self) -> None:
        store = CODEXBOX.host_podman_store(
            {
                "store": {
                    "graphDriverName": "btrfs",
                    "graphRoot": "/var/lib/containers/storage",
                    "graphOptions": {
                        "imagestore": "/srv/containers/images",
                    },
                }
            }
        )

        self.assertIsNotNone(store)
        assert store is not None
        self.assertEqual(store.image_store, "/srv/containers/images")

    def test_additional_image_store_uses_effective_image_store_path(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            home_dir = root / "home"
            graph_root = root / "graph-root"
            image_store = root / "image-store"
            home_dir.mkdir()
            graph_root.mkdir()
            image_store.mkdir()

            additional_store = CODEXBOX.host_podman_additional_image_store(
                {
                    "store": {
                        "graphDriverName": "overlay",
                        "graphRoot": str(graph_root),
                        "graphOptions": {
                            "overlay.imagestore": str(image_store),
                        },
                    }
                },
                home_dir,
                use_fuse_overlayfs=True,
            )

        self.assertEqual(additional_store, str(image_store))


class ForwardedEnvTests(unittest.TestCase):
    def test_repo_ignore_list_does_not_strip_pythonpath(self) -> None:
        patterns = CODEXBOX.load_ignore_patterns(REPO_ROOT / "vars-to-ignore.txt")

        self.assertFalse(CODEXBOX.is_ignored_env_var("PYTHONPATH", patterns))


class PodmanImageSyncTests(unittest.TestCase):
    def test_podman_image_names_ignores_none_entries(self) -> None:
        names = image_sync.podman_image_names(
            {
                "Names": ["localhost/demo:latest", "<none>:<none>"],
                "RepoTags": ["localhost/demo:latest", "localhost/demo:dev"],
            }
        )

        self.assertEqual(
            names,
            ("localhost/demo:latest", "localhost/demo:dev"),
        )

    def test_image_sync_actions_export_new_images_and_tag_added_aliases(self) -> None:
        initial_images = {
            "sha256:existing": image_sync.PodmanImageRecord(
                image_id="sha256:existing",
                names=("localhost/base:latest",),
            )
        }
        current_images = {
            "sha256:existing": image_sync.PodmanImageRecord(
                image_id="sha256:existing",
                names=("localhost/base:latest", "localhost/base:session"),
            ),
            "sha256:new": image_sync.PodmanImageRecord(
                image_id="sha256:new",
                names=("localhost/new:latest", "localhost/new:debug"),
            ),
            "sha256:unnamed": image_sync.PodmanImageRecord(
                image_id="sha256:unnamed",
                names=(),
            ),
        }

        actions = image_sync.image_sync_actions(initial_images, current_images)

        self.assertEqual(
            actions,
            [
                image_sync.ImageSyncAction(
                    image_id="sha256:existing",
                    source_ref="sha256:existing",
                    tag_names=("localhost/base:session",),
                    archive_name=None,
                ),
                image_sync.ImageSyncAction(
                    image_id="sha256:new",
                    source_ref="localhost/new:latest",
                    tag_names=("localhost/new:debug",),
                    archive_name="image-0001.tar",
                ),
                image_sync.ImageSyncAction(
                    image_id="sha256:unnamed",
                    source_ref="sha256:unnamed",
                    tag_names=(),
                    archive_name="image-0002.tar",
                ),
            ],
        )

    def test_image_sync_manifest_round_trips(self) -> None:
        actions = [
            image_sync.ImageSyncAction(
                image_id="sha256:new",
                source_ref="localhost/new:latest",
                tag_names=("localhost/new:debug",),
                archive_name="image-0001.tar",
            ),
            image_sync.ImageSyncAction(
                image_id="sha256:existing",
                source_ref="sha256:existing",
                tag_names=("localhost/base:session",),
                archive_name=None,
            ),
        ]

        with tempfile.TemporaryDirectory() as temp_dir:
            manifest_path = Path(temp_dir) / image_sync.IMAGE_SYNC_MANIFEST_FILENAME
            image_sync.write_image_sync_manifest(manifest_path, actions)

            loaded_actions = image_sync.load_image_sync_manifest(manifest_path)

        self.assertEqual(loaded_actions, actions)


if __name__ == "__main__":
    _ = unittest.main()
