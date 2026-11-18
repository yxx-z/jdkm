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

## 实现中补充的决定（里程碑 2–3）

17. **`uninstall` 必须唯一匹配**：`jdkm uninstall 21` 匹配到多个 21.x 时报错并列出候选，不自动选最新（删除不可撤销）。
18. **`local` / `global` / `shell` 要求版本已安装**，否则报 `JDK 17 is not installed. Run: jdkm install 17`。
19. **空白的 `.java-version` 视为不存在**，继续向上查找。
20. **错误只提示一次的实现**：`jdkm env` 把上次报过的错误存在 shell 变量 `_JDKM_LAST_ERROR` 里，相同就不再输出；问题消失后清除。
21. **`jdkm init zsh` 输出的脚本里写入二进制的绝对路径**（`current_exe()`），钩子不依赖 PATH 查找，也更快。
22. **`jdkm shell` 的实现**：shell 函数以 `JDKM_SHELL=zsh` 调用二进制并 `eval` 其 stdout；直接调用二进制（没装钩子）时报错并提示配置 `.zshrc`。提示信息走 stderr，stdout 只放给 `eval` 的代码。

## 里程碑 4

23. **`list-remote` 不带 `--major` 只列大版本**（用户确认）：1 次 API 调用（adoptium 组织仓库列表），按规则标 LTS（8、11、17 起每 4 个）。
24. **用 tags 接口而不是 releases 列表**：releases 列表每 100 条约 39MB（每个 release 300+ 资产），且**不按版本排序**；tags 每页约 56KB，自己解析排序。GA tag 形如 `jdk-21.0.5+11` / `jdk-21+35` / `jdk-21.0.13.1+2`，EA（`-ea-beta`）和别名（`-ga`）tag 解析失败即跳过。安装时再按 tag 取单个 release（约 640KB）拿资产地址。
25. **双重校验**：下载内容必须匹配 `.sha256.txt`；同时 `.sha256.txt` 必须与 GitHub API 资产的 `digest` 字段一致（有该字段时），否则拒绝安装。
26. **install 候选**：最新的匹配版本若没有本平台构建（respin 有时只发部分平台），依次尝试最多 3 个较旧版本。缓存里找不到匹配版本时自动刷新一次再找。
27. **缓存降级**：远程请求失败但有过期缓存时，警告并使用过期缓存。
28. **GITHUB_TOKEN 只发给 `api.github.com`**，下载文件不带 token。
29. **中断清理**：Ctrl-C 时析构函数不运行，`tmp/` 可能残留；下次 install 开始时删除 `tmp/` 中超过 1 小时的条目。
30. **reqwest 0.13** 中 rustls 特性名为 `rustls`（旧版叫 `rustls-tls`），已确认依赖树中没有 native-tls / openssl。

## 里程碑 5

31. **这一版不做镜像**（用户决定）：TUNA 在当前网络下无法访问（HTTPS 握手失败、HTTP 403），无法按 SPEC 要求实地确认目录结构；GitHub 直连已足够快（200MB 约 20 秒）。因此也不引入 `config.toml` 和 `toml` 依赖。以后要加镜像时的建议方案：config 中配置下载 URL 模板，镜像只用于下载大文件，版本列表和 SHA-256 仍来自 GitHub（镜像内容同样被校验），镜像 404 时回退 GitHub。
32. **钩子状态检测**：init 脚本导出 `JDKM_HOOK=zsh`；`local` / `global` 在钩子未启用时提示配置 `.zshrc`。
33. **网络错误**：连接失败或超时时提示检查网络和代理（`HTTPS_PROXY` / `ALL_PROXY`）。
34. **README 用中文**。
35. **安装时先运行一次 `java -version`**（在 staging 目录里、移入 `versions/` 之前）：确认 JDK 能启动，失败则不安装；同时消化 macOS 首次启动的开销，避免 starship 等提示符工具在 `jdkm local` 后首次运行 `java` 时超时（500ms）。实测安装后首次运行约 6ms。
