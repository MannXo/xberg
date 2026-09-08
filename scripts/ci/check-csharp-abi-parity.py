#!/usr/bin/env python3
"""Guard every C# `DllImport` against the C header the FFI backend actually emitted.

GH#1595. `Registry.SampleBytes()` called a six-parameter native function with three
arguments and declared its `int32_t` status return as a pointer-width `IntPtr`. The call
site null-checked that "pointer", so a success status of 1 passed the guard, and
`Marshal.PtrToStringUTF8` then dereferenced address 1 before `FreeString` freed it. Go
declares the same function correctly, so the disagreement was the C# emitter rendering a
bytes-returning function with the string-returning template.

Nothing could have caught it. `packages/csharp` is alef-generated, so it is not reviewed
line by line; `SampleBytes` has no call site anywhere in our tests or e2e, so no suite
executes it; and it compiles cleanly, because a `DllImport` arity is never checked against
anything at build time -- the mismatch is only observable when the call runs.

## The property

For every `EntryPoint = "xberg_..."` in `NativeMethods.cs`, the managed declaration must
match the header declaration in ARITY, in per-parameter width class, and in return width
class. This is deliberately compared against the emitted header rather than a type set the
check builds for itself: a fixture can only catch disagreements about a rule both sides
already share, and so is structurally blind to arity, to return pointer-ness, and to a
function rendered by the wrong template -- which is every part of GH#1595.

## Width classes, not exact spellings

`uintptr_t` and `UIntPtr` are both pointer-width and agree; comparing spellings would
report them and train readers to ignore output. Types this script cannot classify on
either side are skipped rather than guessed, so an unclassifiable pair is never a failure.
That is a deliberate soft edge: this check exists to catch the loud shapes (arity, pointer
vs scalar, 32 vs 64), and a check that guesses would be silenced within a week.

Pointer-width is treated as interchangeable with a fixed 64 bits, which is what makes a
`uintptr_t` return declared as C# `ulong` (seven of them today) legitimate rather than a
finding. That equivalence is TRUE ONLY BECAUSE every shipped runtime identifier is 64-bit,
so it is derived from `runtime.json.template` at run time instead of being assumed. Ship a
32-bit RID and the equivalence is withdrawn automatically and those declarations start
failing -- which is correct, because on that target `ulong` really would be the wrong
width. The alternative, a static waiver list, would have gone quietly stale at exactly the
moment it mattered.

## What is deliberately NOT guarded

Parameter NAMES and exact signedness. The emitters legitimately differ on both -- `this_`
vs `handle`, `int32_t` vs `int` for a bool-ish status -- and encoding that would make this
a change-detector rather than an invariant.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
HEADER = REPO_ROOT / "crates/xberg-ffi/include/xberg.h"
NATIVE_METHODS = REPO_ROOT / "packages/csharp/src/Xberg/NativeMethods.cs"
RUNTIME_TEMPLATE = REPO_ROOT / "packages/csharp/Xberg/runtime.json.template"

# RID architecture suffixes that are 32-bit. A RID whose suffix is absent from BOTH this set
# and the 64-bit set is treated as 32-bit, so an unrecognised architecture tightens the check
# rather than silently relaxing it. ~keep
_ARCH_64 = frozenset({"x64", "arm64"})
_ARCH_32 = frozenset({"x86", "arm"})

# Known-broken pairs that are NOT this script's job to fix: `packages/csharp` is
# alef-generated, so the repair belongs in alef's C# backend and a hand-patch here would be
# reverted by the next regen. Each entry must name its issue. Remove an entry when the
# upstream fix lands -- the check then proves the fix rather than merely asserting it. ~keep
KNOWN_BROKEN = {
    "xberg_registry_sample_bytes": "GH#1595 -- bytes-returning fn rendered with the string template",
}

_HEADER_FN = re.compile(r"([A-Za-z_][\w]*(?:\s+[A-Za-z_][\w]*)*\s*\**)\s*\b(xberg_\w+)\s*\(([^;]*?)\)\s*;", re.DOTALL)
_CSHARP_FN = re.compile(
    r'EntryPoint\s*=\s*"(\w+)"[^;]*?extern\s+([\w\.\*<>\[\]\?]+)\s+\w+\s*\(([^;]*?)\)\s*;', re.DOTALL
)
_ENTRY_POINT = re.compile(r'EntryPoint\s*=\s*"(\w+)"')

POINTER = "ptr"
_WIDTHS: tuple[tuple[str, object], ...] = (
    ("uint64_t", 64),
    ("int64_t", 64),
    ("uintptr_t", POINTER),
    ("intptr_t", POINTER),
    ("size_t", POINTER),
    ("nuint", POINTER),
    ("nint", POINTER),
    ("ulong", 64),
    ("long", 64),
    ("double", 64),
    ("uint32_t", 32),
    ("int32_t", 32),
    ("uint", 32),
    ("float", 32),
    ("int", 32),
    ("uint8_t", 8),
    ("byte", 8),
    ("bool", 8),
)


def split_params(raw: str) -> list[str]:
    parts = [p.strip() for p in raw.split(",") if p.strip()]
    return [] if parts == ["void"] else parts


_SCALAR_TYPEDEF = re.compile(r"typedef\s+(\w[\w ]*?)\s+(\w+)\s*;")


def scalar_typedefs(header_source: str) -> dict[str, str]:
    """Map header typedef names to their underlying scalar spelling.

    `XBERGAlefHandle` is `uint64_t`, and it is the parameter type of very nearly every
    exported function. Leaving it unclassified made `width_class` return None for it, which
    this script treats as "skip" -- so every handle parameter went uncompared and a handle
    declared at the wrong width would have passed silently. That is the exact shape this
    check exists to catch, and a negative control is what exposed it. Resolve typedefs from
    the header rather than hardcoding the name, so a renamed or re-widened handle is picked
    up without editing this script. Opaque `typedef struct X X;` lines resolve to a
    non-scalar and are correctly left unclassified. ~keep
    """
    resolved = {}
    for underlying, name in _SCALAR_TYPEDEF.findall(header_source):
        underlying = underlying.strip()
        if underlying.startswith(("struct", "enum")):
            continue
        resolved[name] = underlying
    return resolved


def width_class(declaration: str, typedefs: dict[str, str] | None = None) -> object | None:
    """Classify a parameter or return type by ABI width, or None when unclassifiable."""
    text = declaration.replace("const", "").strip()
    if "*" in text or "IntPtr" in text or "[]" in text or "string" in text:
        return POINTER
    if typedefs:
        for name, underlying in typedefs.items():
            if re.search(rf"\b{re.escape(name)}\b", text):
                text = f"{text} {underlying}"
                break
    for spelling, width in _WIDTHS:
        if re.search(rf"\b{spelling}\b", text):
            return width
    return None


def shipped_rids() -> list[str]:
    import json

    return sorted(json.loads(RUNTIME_TEMPLATE.read_text())["runtimes"])


def all_shipped_rids_are_64_bit(rids: list[str]) -> tuple[bool, list[str]]:
    """Report whether every shipped RID is 64-bit, naming the ones that are not."""
    not_64 = []
    for rid in rids:
        arch = rid.rsplit("-", 1)[-1]
        if arch in _ARCH_64:
            continue
        not_64.append(rid if arch in _ARCH_32 else f"{rid} (unrecognised arch `{arch}`)")
    return (not not_64), not_64


def widths_agree(left: object, right: object, pointer_is_64: bool) -> bool:
    return left == right or (pointer_is_64 and {left, right} == {POINTER, 64})


def parse_header(source: str) -> dict[str, tuple[str, list[str]]]:
    return {
        match.group(2): (match.group(1).strip(), split_params(match.group(3))) for match in _HEADER_FN.finditer(source)
    }


def parse_csharp(source: str) -> dict[str, tuple[str, list[str]]]:
    return {
        match.group(1): (match.group(2).strip(), split_params(match.group(3))) for match in _CSHARP_FN.finditer(source)
    }


def compare(
    name: str,
    header: tuple[str, list[str]],
    managed: tuple[str, list[str]],
    pointer_is_64: bool,
    typedefs: dict[str, str],
) -> list[str]:
    header_return, header_params = header
    managed_return, managed_params = managed
    if len(header_params) != len(managed_params):
        return [
            f"{name}: arity -- header declares {len(header_params)} parameter(s), C# declares {len(managed_params)}"
        ]
    problems = []
    header_width = width_class(header_return, typedefs)
    managed_width = width_class(managed_return, typedefs)
    if (
        header_width is not None
        and managed_width is not None
        and not widths_agree(header_width, managed_width, pointer_is_64)
    ):
        problems.append(f"{name}: return -- header `{header_return}` vs C# `{managed_return}`")
    for index, (native, csharp) in enumerate(zip(header_params, managed_params, strict=True)):
        native_width = width_class(native, typedefs)
        csharp_width = width_class(csharp, typedefs)
        if native_width is None or csharp_width is None:
            continue
        if not widths_agree(native_width, csharp_width, pointer_is_64):
            problems.append(f"{name}: parameter {index} -- header `{native}` vs C# `{csharp}`")
    return problems


def main() -> int:
    for path in (HEADER, NATIVE_METHODS, RUNTIME_TEMPLATE):
        if not path.is_file():
            print(f"FATAL: {path} not found", file=sys.stderr)
            return 2

    rids = shipped_rids()
    pointer_is_64, not_64 = all_shipped_rids_are_64_bit(rids)
    if pointer_is_64:
        print(f"all {len(rids)} shipped RID(s) are 64-bit; pointer-width == 64 for this check")
    else:
        print(f"32-bit RID(s) shipped ({', '.join(not_64)}); pointer-width is NOT 64")
        print("Declarations spelling a pointer-width native type as a fixed-64 managed type")
        print("are now reported -- on those targets the width genuinely differs.")

    header_source = HEADER.read_text()
    csharp_source = NATIVE_METHODS.read_text()
    typedefs = scalar_typedefs(header_source)
    header_functions = parse_header(header_source)
    csharp_functions = parse_csharp(csharp_source)
    declared = set(_ENTRY_POINT.findall(csharp_source))

    # Coverage is asserted, not assumed. An earlier hand-run of this comparison matched only
    # 210 of 282 entrypoints -- the header pattern required whitespace before the function
    # name and so skipped every `char *xberg_...` declaration -- and still reported the one
    # real defect. A partial scan and a complete one agreeing is luck. A scan that silently
    # examines a subset is the failure mode this check exists to prevent, so refuse to pass
    # rather than report a clean result over an unknown fraction of the surface. ~keep
    comparable = sorted(declared & set(header_functions))
    unmatched = sorted(declared - set(header_functions))
    print(f"C# entrypoints declared: {len(declared)}")
    print(f"header declarations parsed: {len(header_functions)}")
    print(f"compared: {len(comparable)}")
    if unmatched:
        print(f"FATAL: {len(unmatched)} C# entrypoint(s) have no parseable header declaration:")
        for name in unmatched[:20]:
            print(f"  {name}")
        print("Either the symbol does not exist (a real defect) or this script's header")
        print("pattern missed it (a defect in this script). Both must be resolved, not skipped.")
        return 1
    if not comparable:
        print("FATAL: nothing was compared", file=sys.stderr)
        return 2

    failures: list[str] = []
    waived: list[str] = []
    for name in comparable:
        problems = compare(name, header_functions[name], csharp_functions[name], pointer_is_64, typedefs)
        if not problems:
            continue
        if name in KNOWN_BROKEN:
            waived.extend(f"{problem}  [waived: {KNOWN_BROKEN[name]}]" for problem in problems)
        else:
            failures.extend(problems)

    for line in waived:
        print(f"WAIVED  {line}")
    stale = sorted(
        name
        for name in KNOWN_BROKEN
        if name in comparable
        and not compare(name, header_functions[name], csharp_functions[name], pointer_is_64, typedefs)
    )
    if stale:
        print("FATAL: KNOWN_BROKEN entries no longer reproduce and must be removed:")
        for name in stale:
            print(f"  {name} -- {KNOWN_BROKEN[name]}")
        return 1

    if failures:
        print(f"FATAL: {len(failures)} C#/header ABI disagreement(s):")
        for line in failures:
            print(f"  {line}")
        return 1

    print(f"OK: {len(comparable)} declarations agree with the emitted header ({len(waived)} waived finding(s))")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
