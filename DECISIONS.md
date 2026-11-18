# 设计决策记录

对 SPEC.md 中未定或需要选择的地方的决定（2026-11-19 确认）。

## 目录与安装

1. **macOS `Contents/Home`**：安装时只取 `Contents/Home` 的内容作为 `versions/<v>/`，与 SPEC 目录图一致，JAVA_HOME 路径最短。代价：丢掉 `Info.plist`，`/usr/libexec/java_home` 看不到这些 JDK（不影响 jdkm 本身）。
2. **版本目录名带发行版前缀**：如 `versions/temurin-21.0.5+11/`，避免以后加 Zulu 等时同版本号冲突。
3. **首次安装**：若还没有全局默认版本，自动设为默认并打印提示（非交互，方便脚本）。
4. **下载包**：安装成功后删除 tar.gz（每个约 200MB）。
5. **JDK 8**：第一版只支持 11+（JDK 8 的 tag 格式 `jdk8u432-b06` 不同）。
6. **原子安装暂存区**：`$JDKM_HOME/tmp/`，与 `versions/` 同一文件系统，保证 `rename` 是原子的。

## 版本语义

7. **`jdkm local 21`**：`.java-version` 原样写 `21`，运行时匹配本地最新 21.x，便于提交到 git 后跨机器使用。
8. **`jdkm global 21`**：`default` 写解析后的完整版本（如 `21.0.5+11`），`current` 软链接指向确定目标。
9. **`.java-version` 中别的发行版前缀**（如 `zulu-17`）：忽略前缀按 17 匹配 Temurin，`jdkm current` 中提示。

## Shell 行为

10. **取消 shell 级版本**：`jdkm shell --unset`。未加载钩子时直接调用二进制的 `jdkm shell`，提示先配置 `eval "$(jdkm init zsh)"`。
11. **无任何版本可用时**（离开项目且无全局默认）：只清理 jdkm 自己设置的东西——移除 PATH 中的 jdkm 条目；JAVA_HOME 指向 `$JDKM_HOME` 下时 unset。
12. **解析到的版本未安装时**：同一目录 + 同一版本只提示一次（shell 函数记住上次提示），避免每个提示符都刷屏。

## 网络与依赖

13. **网络**：已验证 `api.github.com` 与 Release 资产下载（302 → `objects.githubusercontent.com`）均可直连，镜像放在里程碑 5。
14. **远程列表缓存**：24 小时过期，`list-remote --refresh` 强制刷新。
15. **额外依赖**：`tempfile`（dev-dependency；安装时临时目录也可能用到）。暂不加 `assert_cmd`。
16. **Rust edition**：2024。
