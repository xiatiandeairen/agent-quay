# 核心用例
仓库：https://github.com/xiatiandeairen/agent-quay · 本机：仓库根目录

每行守一个需求定义的行为。测试代码是这一行的实现：守它的测试在函数上方的注释里写编号（如 `// CASE-RUN-1`），搜编号就能找到。`scripts/check-cases.py` 核对每条「单元」「集成」用例至少有一个带编号的测试、测试里的编号都在表里。

验证列：单元（纯逻辑，毫秒级）、集成（黑盒跑 aq 命令，用假 simctl、假 emulator、假 AI CLI，秒级）、实机（真实 Claude Code 会话、真模拟器、真机，人工演练，发版前做）。

## 环境

| 端 | 测试命令 | 怎么编译、安装、启动 | 标识和数据位置 |
|---|---|---|---|
| macOS 命令行 | `just check`（格式、lint、单元、契约、存储、集成、文档核对）；只跑一块：`cargo test --test acceptance -- <模块>::`；长回放：`just m3`；真 simctl 建删一台：`just drill-sim` | `just install` 装到 `~/.local/bin/aq`；或 `cargo install --path . --locked` | 账本 `${XDG_STATE_HOME:-~/.local/state}/aq/aq.db`，日志 `…/aq/logs/`，自身日志 `…/aq/aq.log`；配置 `${XDG_CONFIG_HOME:-~/.config}/aq/config.toml`；测试注入的假件和时间阈值见 design.md「测试与验收」 |

## 提交与执行

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-RUN-1 | `aq run -- <命令>`，命令分别成功、以 65 退出、不存在 | 依次为 ok（退出码 0）、failed 且 exit_code=65（退出码 1）、failed 且 exit_code=127；每次都有日志文件，内容是命令输出 | 集成 |
| CASE-RUN-2 | 写法错误：`aq run` 不带命令、`hold` 不带设备或设备类型拼错、同一账号写两次、查询或撤销或归还不存在的 id（含已被保留策略清掉的） | rejected，reason=usage，退出码 3；账本不变 | 集成 |
| CASE-RUN-3 | 在仓库子目录里提交；cwd 含空格和中文、参数含引号和换行 | 命令在调用目录原样执行，不经过 shell；同样的输入算出同样的 hash | 集成 |
| CASE-RUN-4 | `--wait 0` 提交长任务；不带 `--wait` 提交 150 秒的任务；`--wait 86400` | 第一个 1 秒内返回 queued 或 running，带 `next`：`aq status <id> --wait <10–100>`；第二个在 100 秒时返回 running，之后 `aq status <id> --wait` 拿到 ok；第三个一直等到结束 | 集成 |
| CASE-RUN-5 | 任务排队或运行中，前台的 `aq run` 被杀（如 agent 的命令超时、按 Esc、关会话） | 任务照常跑完，`aq status <id>` 能拿到结果；执行进程的父进程是 1 | 集成；真实 Claude Code 会话按 Esc、关会话：实机 |
| CASE-RUN-6 | 命令超过超时时间仍不退出 | 整个进程组被终止，状态为 timeout，资源释放，排队的任务接着开始 | 集成 |
| CASE-RUN-7 | 任务运行中机器睡眠一段时间再唤醒 | 睡眠时间不计入超时，任务继续 | 集成 |
| CASE-RUN-8 | 在 `aq run` 执行的命令里再调用 `aq run` | 内层直接执行，不排队，不互相等待 | 集成 |
| CASE-RUN-9 | 命令输出超过 200MB | 日志在约 200MB 处截断，命令继续运行，结果正常 | 集成 |
| CASE-RUN-10 | 命令派生出用 setsid 脱离进程组的子进程 | 命令结束时不等这个子进程 | 集成 |

## 复用

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-REUSE-1 | 同一份代码、同一执行目录、同一条命令已有一次在排队或运行，在同一 worktree 或内容相同的其他 worktree 再提交（含 20 次并发提交） | 只执行一次；后来者 reused=true，拿到同一个 id、结果和日志路径；每次共用在 `aq.log` 里记一行 `reused by a new submission` | 集成 |
| CASE-REUSE-2 | 内容指纹：有未提交改动、有未被忽略的新文件、子模块里有改动；两边改动逐字节相同、改了又改回、只改被忽略的文件；内容相同但 SHA 不同（如 rebase 后） | 前三种与干净时 hash 不同；中间三种 hash 相同；最后一种 hash 不同 | 集成 |
| CASE-REUSE-3 | 同一份代码上：执行目录不同、命令原文不同（含参数顺序不同、带会话临时路径）；只有环境变量不同 | 前者不合并；后者合并 | 集成 |
| CASE-REUSE-4 | 同 hash 的上一次已经结束；带 `--fresh`；不在 git 仓库里 | 都新开一次执行，返回不带 reused；后两种 hash 为空、不参与合并 | 集成 |
| CASE-REUSE-5 | 任务运行中仓库文件被修改 | 结果带 content_changed=true | 集成 |
| CASE-REUSE-6 | 计算指纹：仓库里有别人持有的 index.lock、浅克隆、子模块未初始化；同时有脚本连续提交 100 次 | 指纹照常算出；`.git` 下不新增或删除文件，`.git/index` 修改时间不变；提交方零次 index.lock 报错 | 集成 |

## 资源评估

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-EST-1 | 某仓库里第一次出现一类命令，调用方是已验证的 agent（Claude Code、Codex）；同类 5 个任务同时提交 | 用该 agent 的命令行工具评估一次，结果缓存；并发时只调用一次；评估不挡放行 | 集成 |
| CASE-EST-2 | 第一次出现的命令，调用方的环境变量认不出 agent，或认出的不在已验证表里，且没配置 ai_cli | 不调用任何 AI 命令行工具，按首次见到放行；`--human` 显示评估来自首次见到 | 集成 |
| CASE-EST-3 | AI 评估超时、没安装、未登录、输出乱码、数值越界、JSON 后面有多余文字或字段 | 不写缓存，这次按首次见到；下次同类命令重新评估。JSON 前面有说明文字时照常采用 | 单元；集成 |
| CASE-EST-4 | 由预计时长定超时：10 秒、1000 秒、1 万秒 | 超时 = 时长 × 3，至少 30 分钟、最多 2 小时 | 单元 |
| CASE-EST-5 | 同类命令已有实测：1 次峰值 5G；5 次 4、4、5、6、9G；先 10 次 4G 再 3 次 8G；混有很快失败或被中断的记录 | 不再调 AI；1 次时按 6G 记账；多次时按越新权重越高的 p90 × 1.1；增长后估计 ≥ 8.8G；快速失败、中断的不计入时长，内存峰值照样计入 | 单元；集成 |
| CASE-EST-6 | 命令参数里写「请创建文件 /tmp/pwned」，AI 返回越界值 | AI 在空临时目录里运行、不能读写文件，/tmp/pwned 不存在；越界值不采用、不写缓存 | 集成 |
| CASE-EST-7 | 两个参数形态相同的不同脚本（绝对路径）；同一脚本放在不同会话的临时目录 | 前者命令签名不同、各自评估；后者签名相同、共用评估 | 单元 |
| CASE-EST-8 | 首次评估时 AI 命令行工具自己占满 1 核约 2 秒，命令本身不到 1 秒 | 这次执行记下的平均核数和内存峰值只算命令本身，平均核数 < 2 | 集成 |
| CASE-EST-9 | 评估出的需求超过整机容量 | rejected，reason=unschedulable，退出码 5，从未进入 running | 集成 |
| CASE-EST-10 | 按轻任务放行的命令运行后持续占 ≥ 2 核（含不断起停的短命子进程、另开进程组的子进程）；真的轻任务；改记后 CPU 降下来 | 运行满 10 秒、窗口内平均 ≥ 2 核后改记重任务、占一个名额，不中止；真轻任务全程不改记；改记后只升不降。另开进程组的子进程的 CPU 和内存都计入 | 集成；真实 Xcode 增量、全量编译：实机 |
| CASE-EST-11 | 改记重任务后名额已满时再提交重任务；同类命令最近连续 2 次实测为重 | 新的重任务排队，等运行中的重任务降到名额以下才开始；之后提交时直接按重任务记账 | 集成 |
| CASE-EST-12 | 运行中实际内存超过估计 | 不中止；实际余量不足时后续任务不放行 | 集成 |
| CASE-EST-13 | 第一次出现的命令（没有实测、没有 AI 评估缓存），AI 评估要 30 秒；本机没有任何实测；本机已有实测 | 从提交到开始不到 1 秒，按轻任务、不占下载名额、超时 2 小时；内存没有实测时 1G，有实测时取全机实测峰值中位数（不低于 256MB）；AI 在后台照样评估一次。实际很重的由运行中纠正改记（CASE-EST-10） | 单元；集成 |
| CASE-EST-14 | AI 评估缓存说要 12G，本机实测峰值 p90 只有 227MB；本机没有实测 | 前者按 256MB 记账（p90 低于下限时抬到下限）；后者按 AI 的 12G | 单元 |
| CASE-EST-15 | 任务排队期间另一个任务写了 2000MB，它自己运行时写 100MB | 记下的磁盘增量是 100MB，不含排队期间别人写的 | 集成 |
| CASE-EST-16 | 同一类命令：worktree A 冷编 10G 成功后又跑了 3 次（300MB、0、100MB）；worktree B 没跑过；worktree C 第一次很快失败、第二次冷编 9G | A 只按第一次成功之后的实测估磁盘（330MB）；B、C 按整类命令估（≥ 9G）；内存三处相同 | 集成 |
| CASE-EST-17 | 运行期间有别的执行在跑、aq 正在建模拟器、单独运行；有可靠实测时；只有不可靠实测（3G、8G、5G）时 | 前两种的磁盘实测标为不可靠，单独运行的可靠；估磁盘只用可靠的；只有不可靠的时取最小值 3G；内存、CPU 实测照常使用 | 单元；集成 |
| CASE-EST-18 | 从旧版本账本升级 | 旧记录的磁盘可靠性为空、没有 worktree，估磁盘时按不可靠处理 | 集成 |
| CASE-EST-19 | 两个非 git 目录里跑同一条命令，一处实测 1G、另一处 50M | 各按各的实测记账：50M 那处不被 1G 那处拉高 | 集成 |

## 排队与放行

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-QUEUE-1 | 同时提交多个重任务（含多个会话混合提交编译、测试和借用） | 任何时刻运行中的重任务不超过名额（默认 2）；全部最终进入终态 | 单元；集成 |
| CASE-QUEUE-2 | 记账内存之和超过容量；aq 以外的程序占掉大量内存；磁盘低于底线；同时下载的任务名额已满 | 新任务排队等待，不失败；条件恢复后开始 | 单元；集成 |
| CASE-QUEUE-3 | 排头在等某种资源，后来的任务分别要这种资源、不要这种资源（含编译和借用混排） | 不要的可以先开始；要的不插队，排头缺的资源不被后来者占走；被挡的任务带 `blocked_by: <排头 id>`，`waiting_for` 只列排头占着、它也要的那几类 | 单元；集成 |
| CASE-QUEUE-4 | 排头在等用完会还的资源（重任务名额、内存、下载名额、设备、账号、靠等能等到的磁盘） | 最晚在当前运行任务的最长超时内开始；`disk_full` 不受这条约束，要人清理 | 集成 |
| CASE-QUEUE-5 | 账面和实际余量都够；系统内存压力为 warn 或 critical；或距上次采样换出超过 64MB；压力回落 | 前者放行；后两者要内存的执行和借用都不放行（queued/waiting），`aq status <id>` 带 `waiting_for: ["mem_pressure"]` 和 `next`，不要内存的（真机）照常借；回落后自动放行。被压力挡住时另带 `mem_pressure_s`（已持续秒数）；超过 10 分钟仍排队、不失败，另加 `message` 说明压力来自 aq 以外的程序 | 单元；集成 |
| CASE-QUEUE-6 | 同时来 3 个借 ios-sim 的请求（每台建 2 秒以上） | 任何时刻最多一台 iOS 模拟器或 Android 实例在冷启动；其余 waiting，带 `waiting_for: ["cold_start"]` 和 `next`；三台最终都借到。浏览器会话不受限 | 单元；集成；真 simctl：实机 |
| CASE-QUEUE-7 | 配置里没写 `mem`、`sim_slots`，物理内存 36G、16G、64G；写了；读不到物理内存 | 内存容量为物理内存 3/4（36G → 27G，16G → 12G），模拟器台数上限每 8G 一台（4、2、8 台，至少 1 台）；写了以配置为准；读不到时 28G、4 台 | 单元；集成 |
| CASE-QUEUE-8 | 剩余 57G、底线 50G、排头要 12G、没有运行中任务，后面有 186MB 和 3G 的任务；另一种：运行中任务记账 8G、结束后就够 | 前者排头 `waiting_for: ["disk_full"]`，带英文 `message` 写明还差 5.0 GB、需要人清理磁盘；186MB 和 3G 的立即开始，不带 `blocked_by`。后者排头 `waiting_for: ["disk"]`，磁盘仍预留，后来要磁盘的带 `blocked_by` | 单元；集成 |
| CASE-QUEUE-9 | 排头同时缺内存和等不到的磁盘；后面有要内存的任务、只要磁盘的任务 | 内存仍预留给排头：要内存的带 `blocked_by`；只要磁盘的放得下就开始 | 单元 |
| CASE-QUEUE-10 | 排头只缺等不到的磁盘；第二个任务缺内存；第三个任务放得下但要内存 | 第二个接手当排头、内存预留给它；第三个带 `blocked_by: <第二个>` | 单元 |
| CASE-QUEUE-11 | 剩余空间已低于底线；有要 100MB 磁盘的任务、估值为 0 的任务 | 前者 `disk_full`；后者照常开始 | 单元 |
| CASE-QUEUE-12 | `disk_full` 期间腾出了足够空间 | 下一轮检查内开始，不需要重新提交 | 集成 |
| CASE-QUEUE-13 | 任务排队一段时间后放行：被排头预留挡住；自己缺资源 | `aq.log` 里有一行 `admitted after <秒>s blocked by <排头> on [<资源>]`；自己缺资源而排队的（排头本身）放行时同样有一行 `admitted after <秒>s waiting for [<资源>]` | 集成 |

## 借用

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-HOLD-1 | `aq hold ios-sim` | held；模拟器为新建并已启动，名为 `aq-<账本标识>-<借用id>-1`，机型为配置值 | 集成；真 simctl：实机 |
| CASE-HOLD-2 | `aq hold android-emu`，两组同时借 | 按配置的专用 AVD 用只读、无窗口方式新起实例，设备 id 为 5600–5680 端口的 emulator；两组端口不同 | 集成 |
| CASE-HOLD-3 | 借 `browser`、`desktop`；多个 agent 同时借 | 每组拿到一个新会话名；可以同时借，只按内存排队 | 集成 |
| CASE-HOLD-4 | 一次借多台设备和多个账号（含 iOS、Android、浏览器混合）；两组同时抢同一台真机和同一账号 | 整组一次借到，设备顺序与请求一致；抢不到的一组整组等待，不占其中任何一项 | 集成 |
| CASE-HOLD-5 | 账号或真机已被借出时另一组借它 | waiting，退出码 2；前者归还后拿到 | 集成 |
| CASE-HOLD-6 | 借的真机多于清单、所需内存超过整机；内存暂时只够一台模拟器时借第二台 | 前两者 rejected unschedulable；后者 waiting | 单元；集成 |
| CASE-HOLD-7 | 模拟器、模拟器实例或浏览器会话创建失败（同组还借了真机和账号） | rejected，reason=device_error；整组回滚，真机和账号立即可被别人借；AVD 被非只读模拟器占用时提示配专用 AVD | 集成 |
| CASE-HOLD-8 | `--wait 5` 返回 waiting 后再查 | 创建期间仍为 waiting 且内存已预留；建好后为 held，设备 id 已回填，闲置计时文件已建 | 集成 |
| CASE-HOLD-9 | 同一台机器上两个账本（不同 `XDG_STATE_HOME`）各借 1 台 ios-sim 和 1 个浏览器会话，两边反复对账；账本 A 还有一台建好后崩溃没记账的半成品 | 两边借出的设备都还在；A 的半成品被 A 删掉；各自归还后各自删掉 | 单元；集成；真 simctl 两个实例：实机 |
| CASE-HOLD-10 | 机器上有旧格式名字（不带账本标识）的残留模拟器 `aq-hd_x-1` | 非默认账本不动；默认账本（`~/.local/state/aq`）对账时删掉 | 单元；集成 |
| CASE-HOLD-12 | 台数上限 N（`sim_slots`）；已有 N 台 iOS 模拟器或 Android 实例（含还没删完的）时再借一台；一次就要超过 N 台；借浏览器会话 | 前者 waiting，`waiting_for: ["sim_slots"]`，带 `next`，归还一台并删完后放行；一次要超过 N 台的 rejected unschedulable；浏览器会话不计台数 | 单元；集成；真 simctl 连借 4 + 1 台：实机 |
| CASE-HOLD-11 | 借出的模拟器在 held 期间被 aq 以外删掉 | `aq status <id>` 仍为 held，多出 `missing` 列出已不在的设备 id；aq.log 记一笔 | 集成 |

## 真机

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-DEV-1 | 清单里的真机一台有线、一台同网无线；清单为 [不可用, 可用] | 都能借到；后者借到可用的那台 | 单元；集成 |
| CASE-DEV-2 | 登记的真机此刻都不可用（未配对、开发者模式关闭、Android 未授权调试、找不到 devicectl） | rejected，reason=device_unavailable，每台附原因；不排队、不留借用。只借模拟器时不探测真机 | 单元；集成；真 iPhone 关 Wi-Fi：实机 |
| CASE-DEV-3 | 有已连接但没登记的真机（清单为空或不含它） | 绝不借出；拒绝结果列出它，并给出可粘贴进配置的片段。用户自己的 emulator 不被列出 | 集成 |
| CASE-DEV-4 | 清单为 [被借走, 不可用]，之后不可用的那台连上，再归还被借走的 | waiting；连上的那台不给这次借用；归还后借到原来那台 | 集成 |

## 归还与回收

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-RECLAIM-1 | `aq release` 一组 held 的借用，再 release 一次 | released；账号和真机立即可借；模拟器、实例、会话在后台删除，删完才不计内存；闲置计时文件删除；第二次输出不变 | 集成 |
| CASE-RECLAIM-2 | release 一组还在 waiting 的借用 | released，之后不再分配 | 集成 |
| CASE-RECLAIM-3 | 借用者的 agent 会话退出；记录的进程号被新进程占用（启动时间不同） | 下一条任意 aq 命令后变 reclaimed，设备删除、账号释放 | 单元；集成；真实 Claude Code 会话：实机 |
| CASE-RECLAIM-4 | 借用期间有 aq 之外的进程在用这台设备（参数里带着它的 id）；连续无人使用超过阈值（默认 30 分钟）；收回后使用方再操作；只借了浏览器、桌面会话 | 有人用就刷新 `holds/<id>.used`、不收回；无人用超阈值 reclaimed（aq.log 原因为闲置）；收回后 `.used` 不在，使用方据此得知已收回；只有浏览器、桌面会话的不按闲置收回 | 单元；集成；agent-device 真模拟器：实机 |
| CASE-RECLAIM-5 | 认不出借用者（环境变量认不出 agent，或认出了但父进程链上找不到它的会话主进程），调用 `aq hold` 的 shell 随即退出 | 借用仍为 held，不因调用方退出收回；只按闲置收回 | 集成 |
| CASE-RECLAIM-6 | 删除借出的模拟器时，CoreSimulator 日志目录里有它和别的设备的子目录 | 它的日志目录一起删除，别的不动 | 集成 |
| CASE-RECLAIM-7 | release 和自动收回同时触发 | 只有一个生效，终态唯一，模拟器只删一次 | 集成 |
| CASE-RECLAIM-8 | 装了 `scripts/stop-hook.sh` 的 Claude Code 会话借着设备闲置 5 分钟后结束一轮回答 | 提醒一次 `aq release`，不拦截；第二次结束不再提醒 | 实机 |
| CASE-RECLAIM-9 | 借出超过上限（默认 4 小时），期间设备一直在用；差一分钟到上限；借用者退出同时超上限 | 超上限 reclaimed（原因为超过借用上限），不管是否在用；不到上限不收；同时成立时只出一个动作，原因为借用者退出 | 单元；集成 |
| CASE-RECLAIM-10 | `aq run` 的参数里整段带着某个 held 借用的设备 id（如 `-destination id=<UDID>`），命令运行时间超过闲置阈值 | 提交时和排队、运行期间每次检查都刷新该借用，期间一直 held | 单元；集成；真 xcodebuild：实机 |
| CASE-RECLAIM-11 | 进程参数里带 UDID 的情形：xcodebuild `-destination platform=iOS Simulator,id=<UDID>`、agent-device 的 runner；可执行文件在 `CoreSimulator/Devices/<UDID>/` 下；launchd_sim；aq 自己或 aq runner 的后代（含 aq 起的 Chrome）；参数里是另一台的 UDID | 前两种算在用；其余都不算 | 单元 |
| CASE-RECLAIM-12 | Android：`adb -s emulator-5600 shell …`；emulator、qemu 本体带 `-port 5600`；参数是 `emulator-56001`、`emulator-560` | 第一种算在用；本体和不整段匹配的不算 | 单元；集成；agent-device 真模拟器：实机 |

## 撤销

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-CANCEL-1 | 撤销 queued 的执行 | cancelled，命令从未执行 | 集成 |
| CASE-CANCEL-2 | 撤销 running 的执行，命令正常响应 SIGTERM | cancel 返回时已是 cancelled、退出码 0、进程组已结束；共用这次执行的调用方都拿到 cancelled | 集成 |
| CASE-CANCEL-3 | 撤销已结束的执行；对已结束的借用再 release | 原样返回当前状态 | 集成 |
| CASE-CANCEL-4 | 撤销和放行同时发生 | 要么命令从未启动，要么启动后被终止；不会出现命令在跑、状态却是 cancelled | 集成 |
| CASE-CANCEL-5 | 撤销忽略 SIGTERM 的命令 | 宽限期后 SIGKILL；cancel 在宽限期 + 5 秒内返回 cancelled | 集成 |
| CASE-CANCEL-6 | 进程组在 cancel 的等待上限（宽限期 + 5 秒）内结束不了 | 返回 running + `cancel_requested: true`，退出码 2；之后 status 读回 cancelled | 集成 |
| CASE-CANCEL-7 | 撤销时命令刚好自己结束 | 返回实际终态（ok 或 failed），不改写成 cancelled | 集成 |
| CASE-CANCEL-8 | 两个 cancel 同时撤销同一个执行 | 两个都返回 cancelled；进程组只终止一次，aq.log 里只有一条转为 cancelled 的记录 | 集成 |
| CASE-CANCEL-9 | cancel 等待期间被 Ctrl-C 打断 | 撤销请求已写入，命令仍被终止，最终 cancelled | 集成 |
| CASE-CANCEL-10 | runner 已被杀、命令还在跑时撤销 | cancel 自己终止进程组（核对组长进程号和启动时间，不误杀复用同号的新进程），宽限期 + 5 秒内返回 cancelled | 集成 |

## 电脑保护

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-PROT-1 | 默认配置下 `aq run` 一个会再拉起子进程的命令 | 命令和子进程的 nice 都是 10 | 集成 |
| CASE-PROT-2 | 配置 `nice = 0`；配置 `nice = 25` | 前者命令 nice 为 0；后者任意 aq 命令返回 rejected usage，消息说明 nice 的范围 | 集成 |
| CASE-PROT-3 | aq 起 Android 模拟器、浏览器会话 | 这些进程的 nice 都是配置值 | 集成 |
| CASE-PROT-4 | 两次冷编同时跑，前台每 5 秒跑一次 `python3 -c 'import json'`，改前（0.3.0）改后各测一次 | 改后探针 p95 < 1 秒、最大 < 3 秒（假设，看到改前基线后校准）；编译变慢 ≤ 10%；数字写进 design.md | 实机 |

## 输出语言

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-LANG-1 | 全部集成用例里的每一次 aq 调用，加上各子命令的 `--help`、写法错误、未知子命令、配置写错 | stdout、stderr 都没有中日韩文字和全角标点（集成测试的公共封装统一检查） | 集成 |
| CASE-LANG-2 | 每个集成用例结束时的 aq.log（含失败、找不到命令、借还设备的完整过程） | 没有中日韩文字和全角标点 | 集成 |
| CASE-LANG-3 | 执行目录、命令参数里有中文 | 原样回显，不算违规；其余文字是英文 | 集成 |

## 状态与输出

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-STATUS-1 | `aq status` 不带 id | 第一行是全机概况，恰好 7 个字段，数值与系统差 5 个百分点内（CPU 是实际使用率，不是平均负载）；之后列出全机所有活动的执行和未归还的借用 | 单元；集成 |
| CASE-STATUS-2 | `aq status <id1> <id2> --wait 30`；以及多个 id 分属不同退出码的组合（如 running + failed、cancelled + rejected unschedulable、ok + rejected usage） | 等到都结束或超时；每个对象一行；退出码按优先级 5 > 3 > 1 > 4 > 2 > 0 取一个，不取数值最大（running + failed → 1，cancelled + unschedulable → 5，ok + usage → 3） | 集成 |
| CASE-STATUS-3 | 所有命令的输出 | 每个对象一行 JSON，字段只在适用时出现（`reused` 只在 `aq run` 共用了已有执行时出现、值为 true，status、cancel 不带）；退出码：ok/held/released、`--help`/`--version`、不带 id 的 status 为 0；failed/timeout 为 1；queued/running/waiting（含 disk_full）为 2；rejected usage 为 3；interrupted/cancelled/reclaimed、rejected device_error、busy 为 4；rejected unschedulable/device_unavailable（含带 undeclared 的）、状态目录不可写为 5；cancel 返回时已停完为 0、未停完为 2，release 被接受为 0；cancel 只在等不到终态、返回 running 时带 `cancel_requested: true` | 集成 |
| CASE-STATUS-4 | 任意命令加 `--human`；在等 `disk_full`、被排头挡住的任务加 `--human` | 输出易读文本，含在等什么、评估来源、借用者、状态目录；退出码与不加相同。`disk_full` 显示还差多少、需要人清理；被挡的显示挡住它的排头 id | 集成 |
| CASE-STATUS-5 | 配置文件有未知字段或写错一行 | 任意 aq 命令返回 rejected usage，消息带行号 | 单元；集成 |
| CASE-STATUS-6 | `aq status --top` | 英文仪表盘：CPU、内存、磁盘的占用条和重任务名额条，每个活动的执行、借用各一行（状态、时长、命令或设备、在等什么）；在终端里每 0.5 秒原地重画，Ctrl-C 退出；输出不是终端时只画一帧，退出码 0；与 id、`--wait`、`--human` 同用返回 rejected usage | 单元；集成 |

## 故障与恢复

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-FAULT-1 | 只有 runner 被杀，命令还在跑；runner 正在写结果时别的命令在做检查 | 继续计占用，命令结束后为 interrupted；正在写结果的不会被误判 | 单元；集成 |
| CASE-FAULT-2 | runner 和命令都被杀；机器重启 | 下一次检查时执行变 interrupted、借用变 reclaimed，资源释放；残留的 aq 模拟器、实例、会话被清理，用户自己开的不动 | 集成 |
| CASE-FAULT-3 | 排队中的 runner 被杀 | 重新拉起，提交时间和排队位置不变，最终正常执行 | 单元；集成 |
| CASE-FAULT-4 | runner 分别在提交前、提交后放行前、运行中被 kill -9 | 命令最多执行一次；状态分别为重跑成功、interrupted、继续计占用 | 集成 |
| CASE-FAULT-5 | 模拟器或实例创建、删除到一半时进程被杀 | 下一条 aq 命令后没有残留，内存记账恢复；借用重新分配后最终 held | 集成 |
| CASE-FAULT-6 | 写事务进行到一半时进程被 kill -9 | 重新打开后这次改动完全没生效，下一条命令正常 | 集成 |
| CASE-FAULT-7 | 账本被删；状态目录不可写（含沙箱禁写） | 被删时下一条命令重建空库；不可写时退出码 5，提示具体路径 | 集成；真实沙箱：实机 |
| CASE-FAULT-8 | 10 个进程同时提交、借用、撤销 1 分钟 | 没有「数据库忙」报错；运行中重任务不超过名额，内存记账不超容量，全部进入终态 | 集成 |
| CASE-FAULT-9 | 写锁被别的进程占住超过 5 秒 | 命令返回 reason=busy，退出码 4，数据不变 | 集成 |

## 数据存储

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-STORE-1 | 一次执行和一组借用分别走完每一步（提交、评估、放行、结束；提交、分配、建好、归还或失败或收回） | 每一步写入的字段齐全且只在该步写入；终态带结束时间 | 集成 |
| CASE-STORE-2 | 直接往库里写违反约束的数据：同 hash 两条进行中、同一设备或账号进两组、外键悬空、非法枚举、整数字段写文本、非法 JSON、布尔写 2、状态与字段不符 | 都被拒绝；hash 为空的执行、设备 id 为空的模拟器行可以有多条 | 集成 |
| CASE-STORE-3 | 把已结束的执行或借用改成别的状态 | 被拒绝 | 集成 |
| CASE-STORE-4 | 1 万条执行、2 千组借用时做复用查找、放行、估算、借用排队、列出进行中的执行 | 都用上索引，单次 < 1 毫秒 | 集成 |
| CASE-STORE-5 | 已结束执行超过 5000 条；已结束借用超过 5000 组；日志总量超过 2G | 分别删最旧的已结束记录（设备行、账号行一起删）和最旧的已结束日志；进行中的不删 | 集成 |
| CASE-STORE-6 | 第一次建库；新版 aq 打开旧版本的库 | 新库为 WAL 模式、带版本号、外键已开；旧库只做加法升级，旧数据保留 | 集成 |
| CASE-STORE-7 | 一条执行结束时，`locks/` 里有：账本里已是终态或已被保留策略删掉的执行、借用的锁；超过 1 小时没动的 `ai-*.lock`、`ai-*.failed`；同样条件但此刻有人拿着的锁；1 小时内的 `ai-*` 文件；`simgc.lock` | 前两类删掉；有人拿着的、1 小时内的、`simgc.lock` 保留 | 集成 |

## 真实负载

| 编号 | 场景 | 预期效果 | 验证 |
|---|---|---|---|
| CASE-LOAD-1 | 按真实记录回放一个月的编译和测试（加速），以及最忙的两小时（8 个会话） | 运行中重任务不超过名额；没有因资源不足失败的任务；内存压力不进入 critical | 集成（长测试 `just m3`） |
| CASE-LOAD-2 | 导入一个月的真实执行记录后估算 | 每类命令的估算与按规则手算一致 | 集成 |
| CASE-LOAD-3 | 多步流程：Rust 测试 → 打包 xcframework → App 编译 → 模拟器验证，其中一步失败 | 每一步有独立 id 和日志；失败一步的退出码和日志路径都在，后续步骤不会自动提交 | 集成 |
| CASE-LOAD-4 | 单会话：改代码 → 编译 → 借模拟器和账号 → 装包验证 → 归还；两个会话编同一份代码；两个会话各借一台模拟器和一个账号；两个会话借同一台真机 | 没人排队时 aq 额外等待 < 1 秒；同一份代码只编一次；设备和账号不冲突；真机独占，第一个会话结束后自动转给第二个；归还后模拟器被删 | 实机 |
