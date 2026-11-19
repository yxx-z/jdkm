# jdkm — 用 Rust 写的 JDK 版本管理工具

## 背景

- 我需要在 macOS（Apple Silicon，Mac mini M6）上管理多个 JDK 版本。
- SDKMAN 在我的网络环境下装不上：`get.sdkman.io` 握手失败（`curl: (35) LibreSSL SSL_connect: SSL_ERROR_SYSCALL`），但 `github.com` 可以正常访问。
- 所以这个工具要：**JDK 从 GitHub Releases 下载**，HTTP 用 **rustls**（不依赖系统 LibreSSL），并且支持配置镜像源。
- 我会 Java，Python 懂一点，**Rust 是新手**。这个项目也是我学 Rust 的练手项目（见文末「协作方式」）。

## 目标

1. 安装、卸载、列出 JDK（先只支持 Eclipse Temurin）。
2. 三级版本切换：**全局默认**、**按目录（项目）**、**仅当前终端**。
3. 不同文件夹可以用不同 JDK，互不影响。例如 `~/projA` 用 17、`~/projB` 用 21，其他目录用全局默认。
4. 切换时正确设置 `JAVA_HOME` 和 `PATH`（Maven/Gradle 依赖 `JAVA_HOME`）。

## 非目标（第一版不做）

- Windows / Linux 支持（但代码结构不要写死，平台相关的部分集中在一处）
- Temurin 以外的发行版（用 trait 抽象下载源，方便以后加 Zulu、Corretto 等）
- shim 机制（给 IDE、非交互脚本用），留到后续版本
- bash / fish 支持（第一版只做 zsh，但 `init` 子命令按 shell 名区分，方便扩展）

## 命令设计

```
jdkm list-remote [--major 21]   列出可安装的版本
jdkm install <version>          安装，如 21 / 17 / 21.0.5+11
jdkm uninstall <version>
jdkm list                       列出已安装版本，标出当前生效的版本
jdkm global <version>           设置全局默认版本
jdkm local <version>            在当前目录写入 .java-version
jdkm shell <version>            仅当前终端生效（需要 shell 钩子配合）
jdkm current                    显示当前生效版本及来源（shell / 哪个 .java-version 文件 / global）
jdkm which <cmd>                显示 java、javac 等命令的实际路径
jdkm env                        输出当前目录应使用的 export 语句（给钩子调用）
jdkm init zsh                   输出要加到 .zshrc 的钩子脚本
```

- `<version>` 只写大版本号（如 `21`）时，安装该大版本的最新版；`list`、`local` 等命令中的 `21` 匹配本地已安装的最新 21.x。
- 要用的版本没装时，给出明确提示：`JDK 17 is not installed. Run: jdkm install 17`。

## 版本解析规则（核心）

优先级从高到低：

1. 环境变量 `JDKM_VERSION`（由 `jdkm shell` 设置）
2. 从当前目录**逐级向上**查找 `.java-version` 文件，取最近的一个
3. 全局默认：`~/.jdkm/default`
4. 都没有：不修改环境，`jdkm current` 提示未设置

`.java-version` 格式：一行版本号，如 `17` 或 `21.0.5`。兼容 jenv 风格的 `temurin-17` 这类带前缀的写法（取其中的版本号部分）。忽略首尾空白和换行。

## Shell 集成（zsh）

用户在 `~/.zshrc` 中加入：

```zsh
eval "$(jdkm init zsh)"
```

`jdkm init zsh` 输出的脚本要做到：

- 注册 zsh 的 `chpwd` 和 `precmd` 钩子：切目录时立即刷新，显示提示符前也刷新，使 `jdkm local` 不切目录仍生效。
- 定义一个 `jdkm` shell 函数包装真实二进制：遇到 `jdkm shell <v>` 时在当前 shell 里 `export JDKM_VERSION=...`，其他子命令直接透传给二进制。
- `jdkm env` 的输出必须**幂等**：先从 PATH 中移除旧的 `~/.jdkm/versions/*/bin` 条目，再把新的放到最前面，重复执行不会让 PATH 越来越长。
- 版本没变化时尽量不输出、不改动。`jdkm env` 每次提示符都会执行，必须足够快（目标 < 10ms），不能访问网络。

## 目录结构

```
~/.jdkm/
├── versions/
│   ├── 17.0.13+11/        ← 直接就是 JAVA_HOME（已经去掉 macOS 的 Contents/Home 层级）
│   └── 21.0.5+11/
├── current -> versions/21.0.5+11   ← 指向全局默认版本的软链接，给 IDE 用
├── default                ← 全局默认版本号
├── cache/                 ← 下载缓存、远程版本列表缓存
└── config.toml            ← 配置（镜像源等）
```

允许用环境变量 `JDKM_HOME` 覆盖 `~/.jdkm`（也方便测试）。

**macOS 注意事项**：Temurin 的 macOS tar 包解压后的结构是 `jdk-21.0.5+11/Contents/Home/...`。真正的 JAVA_HOME 是 `Contents/Home`，安装时要处理好这一层（可以保留整个包然后指向 `Contents/Home`，也可以只取 `Contents/Home` 的内容，选一种并说明理由）。

## 下载源

- **官方元数据**：通过 Temurin 的 GitHub 仓库或 Adoptium API 确定精确版本、平台、文件名及 SHA-256。
- 未登录调用 GitHub API 有频率限制（每小时 60 次），要求：
  - 远程版本列表缓存到 `~/.jdkm/cache/`，设置合理的过期时间
  - 如果有环境变量 `GITHUB_TOKEN` 就带上
- **下载**：LTS 版本同时使用 GitHub 和清华 TUNA（TUNA 只有各 LTS 的最新构建），先竞速选源；中断时断点续传，变慢时中途改用更快的源，最终按官方 SHA-256 校验。不要求用户配置镜像。
- 架构和平台从当前系统检测（aarch64 / x64，mac），不要写死。

## 安装流程要求

1. 下载到临时文件，显示进度条
2. 校验 SHA256，不匹配就报错并删除
3. 解压到临时目录，确认结构正确后**原子地**移动到 `versions/` 下（中途失败不能留下半个版本）
4. 安装完成后，如果还没有全局默认版本，询问或自动设为默认（选一种并说明）
5. `uninstall` 正在作为全局默认使用的版本时，给出警告

## 技术选型

- `clap`（derive 风格）命令行解析
- `reqwest`（**blocking + rustls-tls**，关闭默认的 native-tls）
- `serde` / `serde_json` / `toml` 配置和 API 解析
- `flate2` + `tar` 解压
- `sha2` 校验
- `indicatif` 进度条
- `anyhow` 处理错误，需要区分错误类型的地方用 `thiserror`
- `dirs` 获取 home 目录

依赖尽量少，不确定的地方先问我。

## 里程碑（每完成一个先停下来，给我演示和讲解，然后我说继续）

1. **骨架**：`cargo new`，clap 定义所有子命令（先空实现），配置好目录结构和 `JDKM_HOME`。
2. **本地管理**：`list`、`global`、`current`、`uninstall`、`which`。可以先手动解压一个 JDK 到 `versions/` 来测试。
3. **版本解析和 shell 集成**：`local`、`shell`、`env`、`init zsh`，实现完整的优先级规则。这是核心，要写单元测试（用临时目录模拟多层嵌套的 `.java-version`）。
4. **远程安装**：`list-remote`、`install`，包括 GitHub API、缓存、进度条、SHA256 校验、原子安装、macOS 目录结构处理。
5. **完善**：镜像配置、错误提示、README（安装和 `.zshrc` 配置说明）、`cargo clippy` 零警告。

## 验收场景

```zsh
jdkm install 17 && jdkm install 21
jdkm global 21
cd ~/projA && jdkm local 17
java -version            # 17
cd ~/projA/sub/dir
java -version            # 17（继承上层的 .java-version）
echo $JAVA_HOME          # ~/.jdkm/versions/17.x...
cd ~/projB && jdkm local 21
java -version            # 21
cd ~
java -version            # 21（全局默认）
jdkm shell 17
java -version            # 17（仅当前终端）
mvn -v                   # Maven 识别到的 Java 版本和上面一致
```

## 协作方式

- 我是 Rust 新手，熟悉 Java。每个里程碑结束时，用 Java 类比讲解本阶段涉及的关键 Rust 概念（所有权/借用、`Result` 和 `?`、trait、模块组织等），简洁即可。
- 跟我沟通用中文；代码注释和 CLI 输出用英文。
- 每个里程碑完成后用 git 提交一次。
- 不确定的设计决策先问我，不要自己猜。
