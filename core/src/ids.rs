//! 跨库 id 编码（T28）。
//!
//! 拆库之后，`assets.id` / `dirs.id` / `asset_tags.asset_id` 都只是**库内**的局部 id
//! （每个库文件各自从 1 开始）。但界面上一整套逻辑（多选集合、拖拽、缩略图 URL、
//! 断点续翻的游标）都当 id 是全局唯一的。
//!
//! 解决方案：对外只暴露 `gid = (lib_id << 40) | local_id`。
//! - 库内 id 实际远小于 2^40（1 万亿），永远不会溢出到库号那一段；
//! - 前端 JS 的 number 精确到 2^53，3 个库也只有 3.3e12，安全；
//! - **前端一行都不用改**：它拿到的还是"一个大整数 id"。
//!
//! 为什么是 40 位而不是 32 位：局部 id 会随扫描一直涨（一百万个素材很正常），
//! 32 位（42 亿）现在还够，但没必要给自己留一个十年后才会撞上的天花板。

/// 局部 id 占的位数
pub const LIB_SHIFT: u32 = 40;
/// 局部 id 的掩码
pub const LOCAL_MASK: i64 = (1i64 << LIB_SHIFT) - 1;

#[inline]
pub fn gid(lib: i64, local: i64) -> i64 {
    (lib << LIB_SHIFT) | (local & LOCAL_MASK)
}

#[inline]
pub fn ungid(g: i64) -> (i64, i64) {
    (g >> LIB_SHIFT, g & LOCAL_MASK)
}

#[inline]
pub fn lib_of(g: i64) -> i64 {
    g >> LIB_SHIFT
}

/// 把一批全局 id 按库分组：`tag_assets(ids)` 这种"一次给一堆 id"的命令，
/// 拆库后必须按库分别执行（每个库是独立连接）。
pub fn group_by_lib(ids: &[i64]) -> Vec<(i64, Vec<i64>)> {
    let mut out: Vec<(i64, Vec<i64>)> = Vec::new();
    for &g in ids {
        let (lib, local) = ungid(g);
        match out.iter_mut().find(|(l, _)| *l == lib) {
            Some((_, v)) => v.push(local),
            None => out.push((lib, vec![local])),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_grouping() {
        for (lib, local) in [(1, 1), (2, 42), (3, 1_000_000), (1, LOCAL_MASK)] {
            let g = gid(lib, local);
            assert_eq!(ungid(g), (lib, local), "编解码必须一一对应");
            assert_eq!(lib_of(g), lib);
        }
        // 同一库内 gid 的相对大小 == 局部 id 的相对大小（keyset 分页靠这条）
        assert!(gid(1, 5) < gid(1, 6));
        // 不同库：库号大的整体更大
        assert!(gid(1, LOCAL_MASK) < gid(2, 1));

        let groups = group_by_lib(&[gid(1, 1), gid(2, 7), gid(1, 2)]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], (1, vec![1, 2]));
        assert_eq!(groups[1], (2, vec![7]));
    }
}
