import struct, sys

def cmap(path):
    d = open(path, 'rb').read()
    n = struct.unpack('>H', d[4:6])[0]
    tables = {}
    for i in range(n):
        tag, _, off, ln = struct.unpack('>4sIII', d[12 + 16 * i:28 + 16 * i])
        tables[tag.decode()] = (off, ln)
    off = tables['cmap'][0]
    nsub = struct.unpack('>H', d[off + 2:off + 4])[0]
    codes = set()
    for i in range(nsub):
        pid, eid, so = struct.unpack('>HHI', d[off + 4 + 8 * i:off + 12 + 8 * i])
        s = off + so
        fmt = struct.unpack('>H', d[s:s + 2])[0]
        if fmt == 4:
            segx2 = struct.unpack('>H', d[s + 6:s + 8])[0]
            segs = segx2 // 2
            ends = struct.unpack('>%dH' % segs, d[s + 14:s + 14 + segx2])
            starts = struct.unpack('>%dH' % segs, d[s + 16 + segx2:s + 16 + 2 * segx2])
            for a, b in zip(starts, ends):
                if a != 0xFFFF:
                    codes.update(range(a, b + 1))
        elif fmt == 12:
            ng = struct.unpack('>I', d[s + 12:s + 16])[0]
            for g in range(ng):
                a, b, _ = struct.unpack('>III', d[s + 16 + 12 * g:s + 28 + 12 * g])
                codes.update(range(a, b + 1))
    return codes, sorted(tables)

tests = {
    'vietnamese precomposed': 'ạăâđêôơưỹẫệ',
    'combining marks': '\u0300\u0301\u0303\u0309\u0323',
    'latin-ext': 'łşžő',
    'cyrillic': 'Жд',
    'greek': 'λΩ',
    'arrows': '→←↑',
    'ellipsis/emdash/bullet/times/middot': '…—•×·',
    'digits': '0123456789',
}
for p in sys.argv[1:]:
    c, t = cmap(p)
    print(p.split('\\')[-1], 'codepoints', len(c), 'fvar' in t, 'GSUB' in t, 'GPOS' in t)
    for k, s in tests.items():
        miss = [ch for ch in s if ord(ch) not in c]
        print('   %-40s %s' % (k, 'ok' if not miss else 'MISSING ' + ' '.join('U+%04X' % ord(m) for m in miss)))
