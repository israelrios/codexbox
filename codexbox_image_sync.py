import json
import signal
import subprocess
import sys
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path
from types import FrameType
from typing import Callable, TypeAlias, cast


CONTAINER_RUNTIME = "podman"
IMAGE_SYNC_MANIFEST_FILENAME = "manifest.json"


@dataclass(frozen=True)
class PodmanImageRecord:
    image_id: str
    names: tuple[str, ...]


@dataclass(frozen=True)
class ImageSyncAction:
    image_id: str
    source_ref: str
    tag_names: tuple[str, ...]
    archive_name: str | None


SignalHandler: TypeAlias = (
    signal.Handlers
    | int
    | Callable[[int, FrameType | None], object]
    | None
)


def ordered_unique(values: Iterable[str]) -> list[str]:
    seen: set[str] = set()
    result: list[str] = []
    for value in values:
        if value in seen:
            continue
        seen.add(value)
        result.append(value)
    return result


def require_string_list(value: object, context: str) -> list[str]:
    if not isinstance(value, list):
        raise ValueError(f"Expected {context} to be a list of strings")

    raw_items = cast(list[object], value)
    items: list[str] = []
    for item in raw_items:
        if not isinstance(item, str):
            raise ValueError(f"Expected {context} to be a list of strings")
        items.append(item)
    return items


def require_string(value: object, context: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"Expected {context} to be a string")
    return value


def require_string_key_dict(value: object, context: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ValueError(f"Expected {context} to be a JSON object")

    raw_items = cast(dict[object, object], value)
    result: dict[str, object] = {}
    for key, item in raw_items.items():
        if not isinstance(key, str):
            raise ValueError(f"Expected {context} to contain only string keys")
        result[key] = item
    return result


def run_command(command: list[str], env: dict[str, str] | None = None) -> int:
    completed = subprocess.run(command, env=env, check=False)
    return completed.returncode


def run_json_command(
    command: list[str],
    context: str,
    env: dict[str, str] | None = None,
) -> object:
    try:
        completed = subprocess.run(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
            check=False,
        )
    except FileNotFoundError as exc:
        raise RuntimeError(f"Missing command while querying {context}: {command[0]}") from exc

    if completed.returncode != 0:
        message = completed.stderr.strip() or f"Unable to query {context}."
        raise RuntimeError(message)

    try:
        return cast(object, json.loads(completed.stdout.strip() or "[]"))
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"Unable to parse {context} output.") from exc


def load_podman_images(env: dict[str, str] | None = None) -> dict[str, PodmanImageRecord]:
    raw_images = run_json_command(
        [CONTAINER_RUNTIME, "image", "ls", "--format", "json"],
        "Podman images",
        env=env,
    )
    if not isinstance(raw_images, list):
        raise RuntimeError("Expected Podman images to be a JSON array.")

    image_entries = cast(list[object], raw_images)
    images: dict[str, PodmanImageRecord] = {}
    for index, raw_image in enumerate(image_entries):
        try:
            image_data = require_string_key_dict(raw_image, f"Podman image entry {index}")
            image_id = require_string(
                image_data.get("Id"),
                f"Podman image id for entry {index}",
            ).strip()
        except ValueError as exc:
            raise RuntimeError(str(exc)) from exc
        if not image_id:
            continue

        images[image_id] = PodmanImageRecord(
            image_id=image_id,
            names=podman_image_names(image_data),
        )
    return images


def podman_image_names(image_data: dict[str, object]) -> tuple[str, ...]:
    names: list[str] = []
    for key in ("Names", "RepoTags"):
        raw_names = image_data.get(key)
        if not isinstance(raw_names, list):
            continue
        for raw_name in cast(list[object], raw_names):
            if not isinstance(raw_name, str):
                continue
            name = raw_name.strip()
            if not name or "<none>" in name:
                continue
            names.append(name)
    return tuple(ordered_unique(names))


def image_sync_actions(
    initial_images: dict[str, PodmanImageRecord],
    current_images: dict[str, PodmanImageRecord],
) -> list[ImageSyncAction]:
    actions: list[ImageSyncAction] = []
    archive_index = 1
    for image_id in sorted(current_images):
        current = current_images[image_id]
        initial = initial_images.get(image_id)
        if initial is None:
            source_ref = current.names[0] if current.names else image_id
            tag_names = current.names[1:] if current.names else ()
            actions.append(
                ImageSyncAction(
                    image_id=image_id,
                    source_ref=source_ref,
                    tag_names=tag_names,
                    archive_name=f"image-{archive_index:04d}.tar",
                )
            )
            archive_index += 1
            continue

        added_names = tuple(name for name in current.names if name not in initial.names)
        if not added_names:
            continue
        actions.append(
            ImageSyncAction(
                image_id=image_id,
                source_ref=image_id,
                tag_names=added_names,
                archive_name=None,
            )
        )
    return actions


def write_image_sync_manifest(manifest_path: Path, actions: Iterable[ImageSyncAction]) -> None:
    data = {
        "images": [
            {
                "image_id": action.image_id,
                "source_ref": action.source_ref,
                "tag_names": list(action.tag_names),
                "archive_name": action.archive_name,
            }
            for action in actions
        ]
    }
    _ = manifest_path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def load_image_sync_manifest(manifest_path: Path) -> list[ImageSyncAction]:
    try:
        raw_data = cast(object, json.loads(manifest_path.read_text(encoding="utf-8")))
    except (OSError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"Invalid image sync manifest {manifest_path}: {exc}") from exc

    try:
        data = require_string_key_dict(raw_data, f"image sync manifest {manifest_path}")
        raw_images_obj = data.get("images", [])
        if not isinstance(raw_images_obj, list):
            raise ValueError(f"Expected image sync manifest {manifest_path} images to be a list")
        raw_images = cast(list[object], raw_images_obj)
    except ValueError as exc:
        raise RuntimeError(str(exc)) from exc

    actions: list[ImageSyncAction] = []
    for index, raw_image in enumerate(raw_images):
        try:
            image_data = require_string_key_dict(
                raw_image,
                f"image sync manifest entry {index}",
            )
            image_id = require_string(
                image_data.get("image_id"),
                f"image sync manifest entry {index} image_id",
            ).strip()
            source_ref = require_string(
                image_data.get("source_ref"),
                f"image sync manifest entry {index} source_ref",
            ).strip()
            tag_names = tuple(
                require_string_list(
                    image_data.get("tag_names", []),
                    f"image sync manifest entry {index} tag_names",
                )
            )
            archive_name_raw = image_data.get("archive_name")
            if archive_name_raw is None:
                archive_name = None
            else:
                archive_name = require_string(
                    archive_name_raw,
                    f"image sync manifest entry {index} archive_name",
                ).strip()
                if not archive_name:
                    archive_name = None
        except ValueError as exc:
            raise RuntimeError(str(exc)) from exc

        actions.append(
            ImageSyncAction(
                image_id=image_id,
                source_ref=source_ref,
                tag_names=tag_names,
                archive_name=archive_name,
            )
        )
    return actions


def export_image_sync_actions(sync_dir: Path, actions: list[ImageSyncAction]) -> int:
    sync_dir.mkdir(parents=True, exist_ok=True)
    for action in actions:
        if action.archive_name is None:
            continue
        archive_path = sync_dir / action.archive_name
        print(
            f"Exporting nested Podman image {action.image_id} to {archive_path.name}.",
            file=sys.stderr,
        )
        export_status = run_command(
            [
                CONTAINER_RUNTIME,
                "save",
                "--format",
                "docker-archive",
                "--output",
                str(archive_path),
                action.source_ref,
            ]
        )
        if export_status != 0:
            raise RuntimeError(f"Failed to export image {action.source_ref}.")

    write_image_sync_manifest(sync_dir / IMAGE_SYNC_MANIFEST_FILENAME, actions)
    return len(actions)


def import_image_sync_actions(sync_dir: Path, runtime_env: dict[str, str]) -> int:
    manifest_path = sync_dir / IMAGE_SYNC_MANIFEST_FILENAME
    if not manifest_path.is_file():
        return 0

    actions = load_image_sync_manifest(manifest_path)
    for action in actions:
        if action.archive_name is not None:
            archive_path = sync_dir / action.archive_name
            if not archive_path.is_file():
                raise RuntimeError(f"Missing synced image archive: {archive_path}")
            print(f"Importing synced image archive {archive_path.name}.", file=sys.stderr)
            load_status = run_command(
                [CONTAINER_RUNTIME, "load", "-i", str(archive_path)],
                env=runtime_env,
            )
            if load_status != 0:
                raise RuntimeError(f"Failed to import synced image archive {archive_path}.")

        for tag_name in action.tag_names:
            tag_status = run_command(
                [CONTAINER_RUNTIME, "tag", action.source_ref, tag_name],
                env=runtime_env,
            )
            if tag_status != 0:
                raise RuntimeError(
                    f"Failed to tag image {action.source_ref} as {tag_name}."
                )

    return len(actions)


def forwarded_signals() -> tuple[int, ...]:
    signals = [signal.SIGINT, signal.SIGTERM]
    if hasattr(signal, "SIGHUP"):
        signals.append(signal.SIGHUP)
    return tuple(signals)


def run_interactive_command(command: list[str]) -> int:
    child = subprocess.Popen(command)
    previous_handlers: dict[int, SignalHandler] = {}

    def handle_signal(signum: int, _frame: FrameType | None) -> None:
        if child.poll() is None:
            child.send_signal(signum)

    try:
        for signum in forwarded_signals():
            previous_handlers[signum] = signal.getsignal(signum)
            _ = signal.signal(signum, handle_signal)
        return child.wait()
    finally:
        for signum, previous_handler in previous_handlers.items():
            _ = signal.signal(signum, previous_handler)


def ignore_signals() -> dict[int, SignalHandler]:
    previous_handlers: dict[int, SignalHandler] = {}
    for signum in forwarded_signals():
        previous_handlers[signum] = signal.getsignal(signum)
        _ = signal.signal(signum, signal.SIG_IGN)
    return previous_handlers


def restore_signal_handlers(previous_handlers: dict[int, SignalHandler]) -> None:
    for signum, previous_handler in previous_handlers.items():
        _ = signal.signal(signum, previous_handler)


def internal_sync_session_main(argv: list[str]) -> None:
    if not argv:
        raise SystemExit("Missing image sync directory for internal sync session.")

    sync_dir = Path(argv[0])
    codex_args = argv[1:]
    sync_available = True
    initial_images: dict[str, PodmanImageRecord] = {}
    try:
        initial_images = load_podman_images()
    except RuntimeError as exc:
        sync_available = False
        print(
            f"Image sync disabled for this session: {exc}",
            file=sys.stderr,
        )

    command = ["codex", *codex_args]
    exit_code = run_interactive_command(command)
    if not sync_available:
        raise SystemExit(exit_code)

    previous_handlers = ignore_signals()
    try:
        actions = image_sync_actions(initial_images, load_podman_images())
        synced_count = export_image_sync_actions(sync_dir, actions)
    except RuntimeError as exc:
        print(f"Image sync failed inside container: {exc}", file=sys.stderr)
        if exit_code == 0:
            raise SystemExit(1) from exc
        raise SystemExit(exit_code) from exc
    finally:
        restore_signal_handlers(previous_handlers)

    if synced_count:
        print(
            f"Queued {synced_count} nested Podman image change(s) for host sync.",
            file=sys.stderr,
        )
    raise SystemExit(exit_code)
