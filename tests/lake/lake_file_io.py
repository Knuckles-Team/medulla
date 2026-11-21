"""A pyiceberg `FileIO` that reads `lake://` locations over the lake REST listener.

The lake publishes every metadata, manifest and data file at a virtual
`lake://<namespace>/<table>/<path>` location. The listener that serves the catalog
also serves those bytes at `GET /v1/files/<percent-encoded location>`, behind the
catalog bearer. This module is the reader side: the manifest-list, manifest (Avro)
and Parquet decoding stay pyiceberg's own, only the byte fetch is ours.
"""

from __future__ import annotations

import urllib.error
import urllib.parse
import urllib.request

import pyarrow as pa
from pyarrow import fs as pafs
from pyiceberg.io.pyarrow import PyArrowFileIO

_SCHEME = "lake://"


class _LakeHandler(pafs.FileSystemHandler):
    """A read-only pyarrow filesystem whose files are `GET /v1/files/<location>`."""

    def __init__(self, uri: str, token: str | None) -> None:
        self._uri = uri.rstrip("/")
        self._token = token

    def _fetch(self, path: str) -> bytes:
        location = path if path.startswith(_SCHEME) else _SCHEME + path.lstrip("/")
        url = f"{self._uri}/v1/files/{urllib.parse.quote(location, safe='')}"
        request = urllib.request.Request(url)
        if self._token:
            request.add_header("Authorization", f"Bearer {self._token}")
        try:
            with urllib.request.urlopen(request, timeout=30) as reply:
                return reply.read()
        except urllib.error.HTTPError as error:
            if error.code == 404:
                raise FileNotFoundError(location) from error
            raise

    def get_type_name(self) -> str:
        return "lake"

    def normalize_path(self, path: str) -> str:
        return path

    def get_file_info(self, paths):
        infos = []
        for path in paths:
            try:
                size = len(self._fetch(path))
            except FileNotFoundError:
                infos.append(pafs.FileInfo(path, pafs.FileType.NotFound))
            else:
                infos.append(pafs.FileInfo(path, pafs.FileType.File, size=size))
        return infos

    def get_file_info_selector(self, selector):
        return []

    def open_input_file(self, path):
        return pa.BufferReader(self._fetch(path))

    def open_input_stream(self, path):
        return pa.BufferReader(self._fetch(path))

    def _read_only(self, *_args, **_kwargs):
        raise PermissionError("the lake file listener is read-only")

    create_dir = delete_dir = delete_dir_contents = delete_root_dir_contents = _read_only
    delete_file = move = copy_file = open_output_stream = open_append_stream = _read_only


class LakeFileIO(PyArrowFileIO):
    """Reads `lake://` files through the catalog's own listener; writes are refused."""

    def fs_by_scheme(self, scheme: str, netloc: str | None = None):
        if scheme != "lake":
            return super().fs_by_scheme(scheme, netloc)
        return pafs.PyFileSystem(
            _LakeHandler(self.properties["uri"], self.properties.get("token"))
        )

    def new_output(self, location: str):
        raise PermissionError("the lake file listener is read-only")

    def delete(self, location) -> None:
        raise PermissionError("the lake file listener is read-only")
