from __future__ import annotations

import sys
from pathlib import Path

import pytest

# Make the sibling test helpers importable regardless of rootdir layout.
sys.path.insert(0, str(Path(__file__).parent))

from mock_server import MockServer  # noqa: E402


@pytest.fixture
def server(tmp_path):
    """A scripted mock core listening on a fresh Unix socket path."""
    srv = MockServer(str(tmp_path / "core.sock"))
    try:
        yield srv
        srv.join()
    finally:
        srv.close()
