"""Aggregate `twiggy top --format csv` shallow sizes by Rust crate (issue #1087)."""
import csv
import re
import sys
from collections import Counter


def crate_of(name: str) -> str:
    n = name.lstrip('<&*( ')
    n = re.sub(r'^(impl |dyn |mut |const )+', '', n)
    m = re.match(r'([A-Za-z_][A-Za-z0-9_]*)(?:\[[0-9a-f]+\])?::', n)
    if m:
        return m.group(1)
    if name.startswith('data') :
        return '[data]'
    if name.startswith('code['):
        return '[unnamed code]'
    if re.match(r'^(napi_|emnapi_|_emnapi|node_api_)', name):
        return '[emnapi C]'
    return '[other: C/libc/sections]'


for path in sys.argv[1:]:
    c = Counter()
    total = 0
    with open(path, newline='') as f:
        for row in csv.DictReader(f):
            size = int(row['ShallowSize'])
            if row['Name'].startswith('custom section') or 'subsection' in row['Name']:
                continue
            total += size
            c[crate_of(row['Name'])] += size
    print(f'== {path}  total {total}')
    for k, v in c.most_common(22):
        print(f'{v:>9}  {k}')
