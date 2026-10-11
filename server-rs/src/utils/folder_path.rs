// 虚拟文件夹路径的规整(单一出处):技能 / 流程 / 插件三处资产共用同一口径。
//
// folder 是资产自带的**元数据字符串**,不做真实目录(资产仍各自扁平存储:技能的 DB 列、
// 流程的库 JSON 字段、插件的文件内字段)。统一存「A/B」形式:
// - 空串 = 未分组(前端以「未分组」根节点呈现);
// - 去首尾空白与多余斜杠,故 `/A//B/` 与 `A / B` 都落成 `A/B`——同一文件夹不会因为
//   书写差异在库里裂成两份;
// - 不做深度/字符白名单限制:文件夹纯展示,路径派生的合法性校验(禁环、非法字符)
//   在前端移动操作处(`assetFolders.ts`),后端只保证存储形态稳定。
//
// 代际: L1(基础纯函数,零 `crate::` 出边)——与 `image_sniff` 同类。

/// 规整 folder 路径:按 `/` 切段、逐段去空白、丢弃空段后以 `/` 重连。
pub fn normalize_folder_path(raw: &str) -> String {
    raw.split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_canonical_path_unchanged() {
        assert_eq!(normalize_folder_path("A"), "A");
        assert_eq!(normalize_folder_path("A/B"), "A/B");
        assert_eq!(
            normalize_folder_path("写作/草稿/第三幕"),
            "写作/草稿/第三幕"
        );
    }

    #[test]
    fn strips_whitespace_and_redundant_slashes() {
        assert_eq!(normalize_folder_path("/A//B/"), "A/B");
        assert_eq!(normalize_folder_path(" A / B "), "A/B");
        assert_eq!(normalize_folder_path("A///B"), "A/B");
    }

    #[test]
    fn empty_and_blank_mean_ungrouped() {
        assert_eq!(normalize_folder_path(""), "");
        assert_eq!(normalize_folder_path("   "), "");
        assert_eq!(normalize_folder_path("/"), "");
        assert_eq!(normalize_folder_path("//"), "");
    }
}
