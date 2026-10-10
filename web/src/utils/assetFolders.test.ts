// 附件管理虚拟文件夹纯函数测试(IMPHUB 步骤 D)。
// 全部为纯函数,node 环境即可(无 jsdom / 无 Pinia)。
import { describe, expect, it } from 'vitest';
import {
  FOLDER_SEP,
  buildFolderTree,
  entriesInFolder,
  listFolderPaths,
  normalizeFolderPath,
  validateFolderPath,
} from './assetFolders';

describe('normalizeFolderPath', () => {
  it('去首尾空白与首尾斜杠、折叠重复分隔符、逐段 trim', () => {
    expect(normalizeFolderPath('  写作 / 风格  ')).toBe('写作/风格');
    expect(normalizeFolderPath('/写作//风格/')).toBe('写作/风格');
    expect(normalizeFolderPath('写作 / / 风格')).toBe('写作/风格');
  });

  it('丢弃空段与 . / .. 段(路径是纯展示元数据,不允许回退越级)', () => {
    expect(normalizeFolderPath('a/./b')).toBe('a/b');
    expect(normalizeFolderPath('a/../b')).toBe('a/b');
    expect(normalizeFolderPath('../..')).toBe('');
  });

  it('空 / null / undefined 一律归为未分组(空串)', () => {
    expect(normalizeFolderPath('')).toBe('');
    expect(normalizeFolderPath(null)).toBe('');
    expect(normalizeFolderPath(undefined)).toBe('');
    expect(normalizeFolderPath('   ')).toBe('');
  });
});

describe('validateFolderPath', () => {
  it('空串合法(未分组)', () => {
    expect(validateFolderPath('')).toBeNull();
    expect(validateFolderPath('   ')).toBeNull();
  });

  it('正常路径合法', () => {
    expect(validateFolderPath('写作/风格')).toBeNull();
    expect(validateFolderPath('  工具  ')).toBeNull();
  });

  it('含控制字符(换行/制表)报错', () => {
    expect(validateFolderPath('写作\n风格')).toContain('控制字符');
    expect(validateFolderPath('写作\t风格')).toContain('控制字符');
  });

  it('只由 . / .. 组成时明确报错(否则会静默落到未分组)', () => {
    expect(validateFolderPath('..')).toContain('无效');
    expect(validateFolderPath('.')).toContain('无效');
  });
});

describe('buildFolderTree', () => {
  it('未分组条目单独计数,不进树', () => {
    const tree = buildFolderTree([{ folder: '' }, { folder: undefined }, { folder: '写作' }]);
    expect(tree.ungrouped).toBe(2);
    expect(tree.total).toBe(3);
    expect(tree.folders).toHaveLength(1);
    expect(tree.folders[0].name).toBe('写作');
  });

  it('嵌套路径自动补中间层,count 是直接数、total 含子孙', () => {
    const tree = buildFolderTree([
      { folder: 'A' },
      { folder: 'A/B' },
      { folder: 'A/B' },
      { folder: 'A/C' },
    ]);
    expect(tree.folders).toHaveLength(1);
    const a = tree.folders[0];
    expect(a.path).toBe('A');
    expect(a.count).toBe(1); // 直接落在 A 的
    expect(a.total).toBe(4); // 含 B/C 子孙
    expect(a.children.map((c) => c.name)).toEqual(['B', 'C']);
    const b = a.children.find((c) => c.name === 'B')!;
    expect(b.count).toBe(2);
    expect(b.total).toBe(2);
  });

  it('子文件夹按名称排序(深层也排)', () => {
    const tree = buildFolderTree([{ folder: 'z' }, { folder: 'a' }, { folder: 'm/x' }, { folder: 'm/a' }]);
    expect(tree.folders.map((f) => f.name)).toEqual(['a', 'm', 'z']);
    const m = tree.folders.find((f) => f.name === 'm')!;
    expect(m.children.map((c) => c.name)).toEqual(['a', 'x']);
  });

  it('路径规整后相同的条目合并计数(带空白 / 重复斜杠)', () => {
    const tree = buildFolderTree([{ folder: ' A ' }, { folder: 'A/' }, { folder: '/A' }]);
    expect(tree.folders).toHaveLength(1);
    expect(tree.folders[0].count).toBe(3);
  });

  it('空集合 → 空树', () => {
    const tree = buildFolderTree([]);
    expect(tree).toEqual({ ungrouped: 0, folders: [], total: 0 });
  });
});

describe('listFolderPaths', () => {
  it('去重、排序、不含未分组', () => {
    const paths = listFolderPaths([{ folder: 'b' }, { folder: 'a/x' }, { folder: '' }, { folder: 'a' }]);
    expect(paths).toEqual(['a', 'a/x', 'b']);
  });

  it('不含中间层未出现的路径(无空文件夹持久化:只列实际条目所在路径)', () => {
    // 只有 a/b/c 一个条目时,a 与 a/b 是自动补的中间层,但它们确实"存在"(有条目在子孙下)
    // ——但 listFolderPaths 只认条目直接书写的 folder,故此处只列 a/b/c
    expect(listFolderPaths([{ folder: 'a/b/c' }])).toEqual(['a/b/c']);
  });
});

describe('entriesInFolder', () => {
  const items = [
    { name: 'root1', folder: '' },
    { name: 'a1', folder: 'A' },
    { name: 'b1', folder: 'A/B' },
    { name: 'b2', folder: 'A/B' },
    { name: 'c1', folder: 'C' },
  ];

  it("'' 只取未分组", () => {
    expect(entriesInFolder(items, '').map((i) => i.name)).toEqual(['root1']);
  });

  it('取该文件夹及其子孙', () => {
    expect(entriesInFolder(items, 'A').map((i) => i.name)).toEqual(['a1', 'b1', 'b2']);
    expect(entriesInFolder(items, 'A/B').map((i) => i.name)).toEqual(['b1', 'b2']);
  });

  it('前缀相似但不是子孙的不误入(A/B vs A/BB)', () => {
    const more = [...items, { name: 'bb', folder: 'A/BB' }];
    expect(entriesInFolder(more, 'A/B').map((i) => i.name)).toEqual(['b1', 'b2']);
  });

  it('入参路径也走规整(带空白 / 斜杠)', () => {
    expect(entriesInFolder(items, ' A/B ').map((i) => i.name)).toEqual(['b1', 'b2']);
  });
});

describe('FOLDER_SEP', () => {
  it('分隔符为 /', () => {
    expect(FOLDER_SEP).toBe('/');
  });
});
