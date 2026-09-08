# /// script
# dependencies = ["lupa==2.6"]
# ///
"""Run with `uv run scripts/test-lightroom.py` (or Python with lupa installed)."""
from pathlib import Path
import os
from lupa import LuaRuntime

os.chdir(Path(__file__).resolve().parent.parent)
lua = LuaRuntime(unpack_returned_tuples=True)
for path in Path("integrations/Cull.lrplugin").glob("*.lua"):
    lua.execute("assert(load(...))", path.read_text())
lua.execute(Path("tests/lightroom_contract.lua").read_text())
