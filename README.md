# jdkm

用 Rust 写的 JDK 版本管理工具（macOS / zsh）。

- 从 **GitHub Releases** 安装 Eclipse Temurin，HTTP 使用 **rustls**，不依赖系统的 LibreSSL
- 三级版本切换：**全局默认**、**按目录**（`.java-version`）、**仅当前终端**
- 切换时同时设置 `JAVA_HOME` 和 `PATH`，Maven / Gradle 可以直接使用
- 下载后校验 SHA-256，安装过程是原子的，失败不会留下半个版本

## 安装

```bash
curl -fsSL https://raw.githubusercontent.com/yxx-z/jdkm/main/install.sh | sh
```

然后打开一个新终端。安装脚本会：

1. 按 CPU 架构（Apple Silicon / Intel）从 GitHub Releases 下载预编译的二进制，校验 SHA-256 后放到 `~/.jdkm/bin/jdkm`
2. 在 `~/.zshrc` 末尾加一段配置（以 `# >>> jdkm >>>` 开头），重复运行不会重复添加

```zsh
# >>> jdkm >>>
export PATH="$HOME/.jdkm/bin:$PATH"
eval "$(jdkm init zsh)"
# <<< jdkm <<<
```

升级 jdkm 时，重新运行同一行命令即可。如果要安装指定版本：`curl … | JDKM_INSTALL_VERSION=v0.1.0 sh`。

<details>
<summary>从源码安装</summary>

需要 Rust 工具链（`rustup`）：

```bash
git clone https://github.com/yxx-z/jdkm.git && cd jdkm
cargo install --path .        # 安装到 ~/.cargo/bin/jdkm
```

然后在 `~/.zshrc` 末尾手动加上 `eval "$(jdkm init zsh)"`。
</details>

## shell 集成做了什么

`eval "$(jdkm init zsh)"` 会做三件事：

- 注册一个 `precmd` 钩子，每次显示提示符前根据当前目录切换 JDK。`jdkm local` 之后不需要 `cd` 就会生效。
- 定义一个 `jdkm` shell 函数，让 `jdkm shell <版本>` 能修改当前终端的环境。
- 导出 `JDKM_HOOK=zsh`。如果没有配置钩子，`jdkm local/global` 会给出提示。

钩子每次执行约 1～2ms，不访问网络。

> 放在 `.zshrc` 末尾，是为了让 jdkm 的 `PATH` 条目排在最前面。如果你之前用 Homebrew 或其他方式配置过 `JAVA_HOME`，建议删掉那些配置。

## 快速开始

```zsh
jdkm list-remote                 # 可安装的大版本（标注 LTS）
jdkm list-remote --major 21      # 21 的所有正式版本
jdkm install 21                  # 安装 21 的最新版；第一个安装的版本会自动设为全局默认
jdkm install 17.0.13+11          # 安装指定版本

jdkm global 21                   # 全局默认
cd ~/projA && jdkm local 17      # 在当前目录写入 .java-version
jdkm shell 17                    # 只对当前终端生效
jdkm shell --unset               # 取消当前终端的设置

jdkm current                     # 当前生效的版本及来源
jdkm list                        # 已安装的版本，* 标出当前生效的
jdkm which javac                 # 命令的实际路径
jdkm uninstall 17.0.13           # 卸载（必须能唯一确定一个版本）
```

`21` 这样只写大版本号的写法，在 `install` 里表示"远程最新的 21.x"，在 `global`、`local`、`shell` 里表示"本地已安装的最新 21.x"。

## 版本解析规则

优先级从高到低：

1. 环境变量 `JDKM_VERSION`（由 `jdkm shell` 设置）
2. 从当前目录**逐级向上**查找到的第一个 `.java-version`
3. 全局默认：`~/.jdkm/default`
4. 都没有：不设置。如果之前由 jdkm 设置过 `JAVA_HOME` 和 `PATH`，会把它们清理掉；你自己设置的 `JAVA_HOME` 不受影响

`.java-version` 只有一行版本号，如 `17`、`21.0.5`、`21.0.5+11`。也兼容 jenv 风格的 `temurin-17`。首尾的空白会被忽略，空文件视为不存在。建议提交到 git，让团队成员使用同一个大版本。

如果要用的版本没有安装，jdkm 会提示一次（不会在每个提示符都重复）：

```
jdkm: version 17 set by /Users/me/projA/.java-version: JDK 17 is not installed. Run: jdkm install 17
```

## 目录结构

```
~/.jdkm/                        可用环境变量 JDKM_HOME 覆盖
├── bin/jdkm                    jdkm 本身（通过一键脚本安装时）
├── versions/
│   ├── temurin-17.0.21+9/      每个目录本身就是 JAVA_HOME
│   └── temurin-21.0.13.1+2/
├── current -> versions/…       指向全局默认版本的软链接
├── default                     全局默认版本
├── cache/                      远程版本列表缓存（24 小时）
└── tmp/                        安装时的临时目录
```

macOS 的 Temurin 包解压后是 `jdk-21…/Contents/Home/…` 的结构。jdkm 只保留 `Contents/Home` 的内容，所以 JAVA_HOME 路径很短。代价是 `/usr/libexec/java_home` 识别不到这些 JDK，这不影响 jdkm 本身。

## 在 IDE 中使用

IDE 不会经过 shell 钩子。在 IntelliJ IDEA 的 *Project Structure → SDKs* 中添加：

- `~/.jdkm/current`：始终指向全局默认版本，切换 `jdkm global` 后自动跟随
- 或者 `~/.jdkm/versions/temurin-17.…`：固定某个版本

## GitHub API 频率限制

未登录时每小时最多 60 次。jdkm 会把版本列表缓存 24 小时（`list-remote --refresh` 强制刷新），安装一个版本通常只需要 1～4 次请求。如果遇到限制，设置一个 token（不需要任何权限）：

```zsh
export GITHUB_TOKEN=ghp_xxx
```

token 只会发送给 `api.github.com`。

## 常见问题

**`java -version` 没有切换**
运行 `jdkm current` 看解析结果；运行 `echo $JDKM_HOOK`，如果为空，说明没有配置钩子或者没有打开新终端。另外检查 `.zshrc` 里 `eval "$(jdkm init zsh)"` 之后有没有其他设置 `PATH` 或 `JAVA_HOME` 的语句。

**`cannot reach api.github.com`**
检查网络或代理。jdkm 会读取 `HTTPS_PROXY` / `ALL_PROXY` 环境变量。如果之前有过缓存，jdkm 会给出警告并继续使用旧数据。

**安装被中断（Ctrl-C）**
`versions/` 不会受影响。`tmp/` 里的残留文件会在下次 `install` 时自动清理（超过 1 小时的）。

## 卸载 jdkm

删除 jdkm 本身和 `~/.zshrc` 里的配置，**保留**已安装的 JDK：

```bash
curl -fsSL https://raw.githubusercontent.com/yxx-z/jdkm/main/uninstall.sh | sh
```

连同所有已安装的 JDK（整个 `~/.jdkm`）一起删除：

```bash
curl -fsSL https://raw.githubusercontent.com/yxx-z/jdkm/main/uninstall.sh | sh -s -- --all
```

卸载前会把 `~/.zshrc` 备份为 `~/.zshrc.jdkm-backup`，确认没问题后可以删掉这个备份。如果是从源码安装的，再运行一次 `cargo uninstall jdkm`。

## 开发

```bash
cargo test
cargo clippy --all-targets
```

发布新版本：修改 `Cargo.toml` 里的 `version`，提交后打一个同名 tag 并推送。GitHub Actions 会编译 macOS aarch64 和 x86_64 两个二进制，并发布到 Releases。

```bash
git tag v0.1.0 && git push origin v0.1.0
```

设计决策记录在 [DECISIONS.md](DECISIONS.md)，需求在 [SPEC.md](SPEC.md)。
