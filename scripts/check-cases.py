#!/usr/bin/env python3
"""核对用例表 docs/cases.md 和测试代码是否对得上。

用法：scripts/check-cases.py
测试在 #[test] 上方用一行注释标出它守的用例，如 `// CASE-RUN-1, CASE-RUN-2`；
不守需求行为的集成测试标 `// 表外：<原因>`。查五件事，任何一项不过就以 1 退出：
  1. 表里每行四列、编号唯一、格式为 CASE-<领域>-<数字>；
  2. 验证列写了「单元」的用例，至少有一个 crates/ 或 src/ 里的测试标了它；
  3. 验证列写了「集成」的用例，至少有一个 tests/ 里的测试标了它；
  4. 代码里出现的每个 CASE 编号都在表里；
  5. tests/acceptance、tests/storage 里的每个测试都标了 CASE 编号或「表外」。"""
import collections, pathlib, re, sys

root = pathlib.Path(__file__).resolve().parent.parent
ID = re.compile(r"CASE-[A-Z]+-[0-9]+")
errors = []

cases = collections.OrderedDict()
for n, line in enumerate((root / "docs/cases.md").read_text().splitlines(), 1):
    if not line.startswith("| CASE-"):
        continue
    cells = [c.strip() for c in line.strip().strip("|").split("|")]
    if len(cells) != 4:
        errors.append(f"cases.md:{n}：应为四列（编号、场景、预期效果、验证），现在 {len(cells)} 列")
        continue
    cid, verify = cells[0], cells[3]
    if not ID.fullmatch(cid):
        errors.append(f"cases.md:{n}：编号 {cid} 格式不对")
    if cid in cases:
        errors.append(f"cases.md:{n}：编号 {cid} 重复")
    cases[cid] = verify

unit, integ, mentioned = collections.defaultdict(list), collections.defaultdict(list), set()
for f in sorted([*(root / "tests").rglob("*.rs"), *(root / "crates").rglob("*.rs"), *(root / "src").rglob("*.rs")]):
    rel = f.relative_to(root)
    lines = f.read_text().splitlines()
    mentioned.update(ID.findall(f.read_text()))
    for i, line in enumerate(lines):
        if line.strip() != "#[test]":
            continue
        j = i + 1
        while j < len(lines) and not re.search(r"\bfn ", lines[j]):
            j += 1
        name = re.search(r"fn ([a-z0-9_]+)", lines[j]).group(1) if j < len(lines) else "?"
        tag = lines[i - 1].strip() if i > 0 else ""
        ids = ID.findall(tag) if tag.startswith("//") else []
        for cid in ids:
            (integ if rel.parts[0] == "tests" else unit)[cid].append(f"{rel}::{name}")
        if rel.parts[:2] in (("tests", "acceptance"), ("tests", "storage")) and not ids and not tag.startswith("// 表外"):
            errors.append(f"{rel}::{name}：上方没有标 CASE 编号或「表外」")

for cid, verify in cases.items():
    if "单元" in verify and not unit.get(cid):
        errors.append(f"{cid}：验证写了「单元」，但 crates/、src/ 里没有标它的测试")
    if "集成" in verify and not integ.get(cid):
        errors.append(f"{cid}：验证写了「集成」，但 tests/ 里没有标它的测试")
for cid in sorted(mentioned - set(cases)):
    errors.append(f"代码里的 {cid} 不在 docs/cases.md 里")

manual = [c for c, v in cases.items() if "实机" in v]
print(f"{len(cases)} 条用例；单元测试守 {len(unit)} 条，集成测试守 {len(integ)} 条；含实机演练 {len(manual)} 条")
for e in errors:
    print("✗", e)
sys.exit(1 if errors else 0)
