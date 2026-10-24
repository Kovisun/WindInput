# 拼音纠错与正确拼音提示

> 2026-10-23 设计。前身：`fuzzy-pinyin-hint.md`（模糊音正确拼音提示，已合 main、**未正式发布**）。
> 本设计把那份的提示能力**泛化成多来源**，配置挪出 `[schema.pinyin.fuzzy]`，并新增
> 「误读提示」与「拼写纠错（字母换位）」两个来源。未发布 ⇒ 旧键名直接改，不做迁移。

## 1 问题与目标

模糊音提示（`苹(ping)果`）上线后，维护者认为「提示」不该只挂在模糊音下面：

- 拼错音有不止一种来源——模糊音、**字母顺序打颠倒**（`gogn` 想打 `gong`）、**读错音**
  （「调研」读成 tiao yan）。三者都值得告诉用户「正确的是什么」。
- 因此设置端新开分区「**输入 → 纠错与提示**」，把相关开关收在一起，且**细节可自定义**
  （逐条规则开关、自定义规则、标记符、提示来源）。

本轮目标：

1. **提示通用化**：一套「正确拼音提示」机制，来源可选：模糊音 / 误读 / 纠错。
2. **误读提示**：自动推断 + 误读表兜底。
3. **拼写纠错（字母换位）**：规则表逐条开关 + 自定义规则；纠错候选带标记。

## 2 调研结论（2026-10-23）

| | 换位纠错 | 邻键纠错 | 多打/漏打 | 误读提示 | 纠错可见性 | 可配置 |
|---|---|---|---|---|---|---|
| 搜狗 PC | 5 条（ign/img→ing、uei/uen/iou） | 未见 | 有 | 候选内插拼音「炽(chì)热」 | 有，样式不详 | 总开关 + 规则 |
| QQ 拼音 | 7 条逐条勾选：gn/mg→ng、iou/uei/uen、on→ong、ion→iong | 未见 | 未见 | 「摩羯(jié)座」 | 编码区显示改正后拼音 | 逐条 + 恢复默认 |
| 百度 | 有（规则名未公开） | 手机有 | 有 | PC 有 | 不详 | 规则 / 开关 |
| 讯飞 | gn→ng | 手机有 | 有 | 未见 | 红波浪线、倒 V | 多开关 |
| 雾凇 rime-ice | ~20 条 derive，冲突的几条作者主动注释掉 | 无 | 无 | 注释区，Lua 硬编码 236 条 | 无 | 改 yaml/lua |
| 万象 | ~30 条，含雾凇未启用的 | 无 | 无 | 注释区，`cuoyin.dict` ~200 条 | 无 | 改 yaml |
| librime / libime | 自写规则 / 「常见拼写错误」开关 | 同行左右相邻 | 无（编辑距离代码被 `#if 0`） | 无 | 无 | 开关 |

要点：

- **换位都是固定规则表**，没有通用换位。多条规则与合法切分或简拼冲突——雾凇作者注释掉
  `ai→ia`、`uei/iou/uen` 等，原话「x'ai 喜爱被纠错为 xia 下」。
- **误读提示的数据全是人工误读表**，按「文字 + 错误拼音」双重匹配；没有一家做自动推断。
- **防误纠**：纠错大幅降权（librime ×0.01，模糊 ×0.5）；有精确匹配时纠错不进首选；限条数。
- 用户抱怨集中在：纠错候选排太前（librime #1120）、错音提示太吵（雾凇 #431）。

### 2.1 换位规则全集与冲突（用 8105 字表 419 音节逐条验过）

「冲突」= 错误写法本身能切成合法音节或简拼。

| 规则（正确 ← 错误） | 出处 | 冲突 |
|---|---|---|
| ng ← gn（agn/egn/ign/ogn/iagn/uagn/iogn） | QQ、搜狗、讯飞、雾凇、万象、libime | 全拼无；**简拼下**：`xign`=xi+g+n |
| ng ← mg（img 等） | QQ、搜狗 | 简拼：m 首字母 |
| ng ← nag/neg/nig/nog | 雾凇、万象 | **冲突**：nage=na+ge |
| ui/iu/un ← uei/iou/uen | 搜狗、QQ；雾凇注释掉 | **冲突**：liou=li+ou、duei=du+ei、tuen=tu+en |
| ong/iong ← on/ion | QQ、万象、libime | 弱：lone=lo+ne、xion=xi+o+n |
| eng ← en（仅 t/l） | 万象 | **冲突**：tena=te+na |
| zh/ch/sh ← hz/hc/hs、zih/cih/sih | 雾凇、万象 | 简拼：h+zi |
| ia ← ai（qai/xai） | 万象；雾凇注释掉 | 简拼：x'ai 喜爱 |
| ai←ia（w/g/h/k）、ei←ie（w/f/g/h/k/z）、ie←ei（j/q/x）、ao←oa、ou←uo（y/p/f/m）、ui↔iu | 雾凇、万象 | 未发现 |
| iao ← ioa/oia | 雾凇、万象 | 弱：xioa=xi+o+a |
| un ← nu；an/en/in ← na/ne/ni | 万象启用 un←nu；雾凇全注释 | 简拼：dnu=d+nu |
| ua/uai/uang/iang/ue 的各种换位 | 万象 | 未发现 |
| v ↔ u | 雾凇、万象、libime | **本仓已支持**（`pinyin/spelling.rs` `normalize_u_umlaut`），不另立规则 |

来源链接见 §11。

## 3 决策

| # | 决策 | 出处 |
|---|---|---|
| D1 | 本轮做：提示通用化、误读提示、字母换位纠错。**邻键纠错另立项**；**多打/漏打不做** | 维护者定 |
| D2 | 设置端新分区「输入 → 纠错与提示」，模糊音区不再放提示项 | 维护者定 |
| D3 | 配置挪出 `[schema.pinyin.fuzzy]`，旧键（`hint`/`hint_scope`/`hint_tone`）直接删，不迁移 | 维护者定（未发布不兼容，见 AGENTS.md / 历史约定） |
| D4 | 误读判据 = **自动推断 + 误读表兜底**（§6） | 维护者定 |
| D5 | 纠错候选可见性：候选前加**纠错标记**（出厂 `*`，可自定义，留空不加）；编码栏**保持用户原文**（显示 `gogn'ping`，不改写成 `gong'ping`）；内嵌正确拼音作为提示的可选来源，**纠错来源默认不勾** | 维护者定（原话：只改编码栏「用户根本不知道自己有没有输入错误」；参照「放宽候选标记」） |
| D6 | 纠错候选**重降权**；同一输入存在非纠错的**完整候选**时，纠错候选**不得占首选**。「完整候选」= 覆盖全部输入、每个音节都是完整音节（精确或模糊命中）的候选；简拼、前缀补全、末尾半截**不算**——否则 `gogn` 可切 g+o+g+n 出简拼候选，`gn_ng` 永远拿不到首选。纠错与模糊同分时模糊在前 | 调研 §2（librime/libime 做法），本设计的判断 |
| D7 | 换位规则逐条开关；无冲突的出厂开，有冲突的出厂关，设置端每条旁写冲突示例 | 调研 §2.1；QQ 逐条勾选 + 恢复默认 |
| D8 | 支持**自定义换位规则**（§5.3） | 维护者「需要能自定义细节」 |
| D9 | 提示只影响显示；纠错只影响召回与标记。上屏文字、学习写入的编码（`learn_code`）语义不变 | 延续 fuzzy-pinyin-hint D7 |

## 4 配置

### 4.1 形状（字段名为本设计的判断，实现者可微调，但须保持分组）

```toml
# ── 纠错与提示 ─────────────────────────────────────
[input.correction]
# 字母换位纠错总开关
enabled = true
# 纠错候选前加的标记；留空不加
marker = "*"

# 逐条规则开关（id 见 §5.2）
[input.correction.rules]
gn_ng = true
on_ong = true
vowel_swap = true
legacy_spelling = false      # uei/iou/uen
h_misplaced = false          # hz/hc/hs、zih
mg_ng = false
n_before_g = false           # nag/neg/nig/nog
ai_ia = false                # qai/xai、nu→un 等简拼冲突类

# 自定义规则（§5.3）
custom = []

[input.correction.hint]
# 显示正确拼音（候选正文内嵌，如「苹(ping)果」）。出厂关
enabled = false
# 提示来源
fuzzy = true
misread = true
correction = false
# "first" 只标第一个带提示的候选 | "all"
scope = "first"
# 带声调（同时作用于模板变量）
tone = false
```

- 作用域：**全局**（与 `[schema.pinyin.fuzzy]` 同为全局；临拼按 overlay 对齐原则同样生效）。
- 原 D5（`enabled=false` 时 `hint` 不生效）改为**按来源判**：`fuzzy` 来源只在模糊音开时有结果；
  `correction` 来源只在纠错开时有结果；`misread` 与两者无关。
- 共享开关（`HintSwitch`）从 `FuzzyConfig` 拆出，变成独立的提示配置，计算条件：
  `hint.enabled || 任一生效模板引用了正确拼音变量`，再按来源过滤。

### 4.2 改名（全部去 fuzzy 化，未发布直接改）

| 旧 | 新（建议） |
|---|---|
| `[schema.pinyin.fuzzy] hint / hint_scope / hint_tone` | `[input.correction.hint] enabled / scope / tone` |
| `FuzzyHint`、`meta.fuzzy_hints`、`meta.fuzzy_syllables` | `PinyinHint { char_index, correct, kind }`、`meta.pinyin_hints`、`meta.hint_syllables` |
| 主题角色 `fuzzy_hint` | `pinyin_hint` |
| 模板变量 `${fuzzy_pinyin}`、角色 `fuzzy_pinyin` | `${correct_pinyin}`、角色 `correct_pinyin` |
| `fuzzy_hint.rs`、`fuzzy_hint_text.rs`、`fill_fuzzy_hints` 等 | 去 fuzzy 字样 |

`kind` 取值 `fuzzy | misread | correction`。本轮三者**同一颜色**（一个角色）；`kind` 用于按来源过滤与
`scope` 计数，并为以后分色留口子。

改动面（调研估）：核心仓约 10 个文件、wind-setting 6 处、Android manifest/config/测试、文档站 schema 页、
主题编辑器（角色名）、`docs/design/` 6 份引用。

## 5 拼写纠错（字母换位）

### 5.1 挂点

仿照模糊音：在 `fuzzy::fuzzy_variants_scored`（`wind-engine/src/pinyin/fuzzy.rs:506`）旁加「纠错变体」
生成器，同一 `(variant, edits)` 接口：

- 纠错拼写注册进第二棵 trie（`syllable.rs` `load_fuzzy_spellings`），**沿用「只收非法拼写」约束**——
  错误写法若本身是合法音节，不收（防 `tie`→`tei` 这类万象的冲突）。
- 词图层进 `LatticeBuilder::fuzzy_spans`（`lattice.rs:976`）同一路径，真实码记进 `canon` ⇒ `learn_code`、
  学习、提示整条链复用。
- **罚分单列**，明显重于模糊音（参考 librime ×0.01 vs 模糊 ×0.5）；具体值走 `pinyin_eval` 扫参定（§9）。
- 纠错与模糊在同一音节上**不叠加**（一个音节最多一处编辑来源），防组合爆炸。

### 5.2 内置规则（行 = 设置端一个开关）

| id | 设置端文案（草案） | 覆盖 | 出厂 | 冲突示例（写进 hint） |
|---|---|---|---|---|
| `gn_ng` | ng 打成 gn（ogn→ong） | agn/egn/ign/ogn/iagn/uagn/iogn | 关（未过 §9 门禁：F 类简拼首选/首页下降） | 简拼下 xign 也可是 xi+g+n |
| `on_ong` | 漏 g：on→ong | on→ong、ion→iong | 开 | lone 也可是 lo+ne |
| `vowel_swap` | 韵母字母颠倒 | ai←ia（wghk）、ei←ie（wfghkz）、ie←ei（jqx）、ao←oa、ou←uo（ypfm）、ui↔iu、iao←ioa/oia、ua/uai/uang/iang/ue 的换位 | 开 | 无 |
| `legacy_spelling` | 旧式拼写 uei/iou/uen | →ui/iu/un | 关 | liou 也可是 li+ou |
| `h_misplaced` | h 错位（hz→zh、zih→zhi） | hz/hc/hs、zih/cih/sih | 关 | h+zi 是简拼 |
| `mg_ng` | ng 打成 mg | mg→ng | 关 | m 是简拼首字母 |
| `n_before_g` | ng 打成 nag 类 | nag/neg/nig/nog | 关 | nage 也可是 na+ge |
| `ai_ia` | 简拼冲突类 | qai/xai→qia/xia、nu→un、na/ne/ni→an/en/in | 关 | x'ai 喜爱 会被纠成 xia |

- 「出厂开」的三条须过 §9 的评测门禁（打对的输入首页不被纠错候选挤动）后才定为开；不过就降为关并记录。
- 「恢复默认」由设置端现有的恢复机制提供。
- 双拼：本节规则是全拼字母层的，**双拼不适用**（两键即一音节，颠倒属键对层，随邻键纠错另立项）。
  混输的拼音侧随全拼继承。

### 5.3 自定义规则

`custom = ["ogn=ong", "uen=un"]` 形式：`错误写法=正确写法`，作用于**单个音节内**的片段替换
（与内置规则同一生成器、同一罚分、同一「只收非法拼写」约束）。

- 生成方向（本设计的判断）：**从合法音节出发**——对每个含「正确写法」片段的合法音节，把该片段换成
  「错误写法」，得到的错误拼写注册进纠错 trie（与内置规则同一条路，不对用户输入做串替换）。片段可在音节任意位置。
- 某条规则一个可用的错误拼写都生成不出来（全是合法音节或无匹配）⇒ 该条无效，加载时 `warn` 日志，不报错退出。
- 冲突提示：设置端对每条自定义规则跑与 §2.1 同一冲突检查（错误写法能否切成合法音节/简拼），有冲突就在该行
  显示示例（如 `uen=un`：tuen 也可是 tu+en），不阻止保存。
  设置端冲突判据中单元音（a/o/e）不成段，§2.1 的「弱」冲突（如 xioa=xi+o+a）不提示。
- 设置端：「拼写纠错」扩展对话框里列内置规则开关 + 自定义规则列表（增删）。
- 语法取舍（是否支持声母前置条件如 `[jqx]ie=…ei`）：**本轮只做字面片段**，按需再扩（本设计的判断）。

### 5.4 候选标记

- 候选新增事实字段 `is_corrected: bool`（语义只表达「经换位纠错命中」，**不编码排序或显示决策**，
  同 `is_scope_filtered` 的约定，见 `wind-candidate/src/candidate.rs:200`）。
- 消费者：
  - UI：按 `input.correction.marker` 加前缀，与命令前缀、「放宽候选标记」走同一个 `prefix_of`
    （`coordinator.rs` 候选组装处），**与它们一样按正文色画、不单独着色**（本设计的判断：现有前缀都是纯文本，
    单为它做分段着色不划算）。移动端拉取窗口目前不显示任何前缀标注（`candidate_pull.rs`），本轮同样不加，记为后续项；
  - 排序：D6；
  - 自动上屏：计数时跳过（防满码唯一自动上屏被纠错候选打断，同 `is_scope_filtered`）；
  - 学习/调频：正常（用户选了就是要这个词），写入 `learn_code`。
- 编码栏：保持用户原文与原切分（D5）。⚠️ 需钉测试：纠错命中时 preedit 仍是 `gogn'ping` 形态
  （见本仓约定「拼音切分改动要测编码栏」）。
- 整句里只有部分音节被纠也算 `is_corrected`。
- 去重（本设计的判断）：同一文字既能经纠错又能经非纠错路径得到时，**保留非纠错那条**，不加标记。因此 `*`
  的含义是「这个候选只有经纠错才得到」；字段文档按此措辞。

## 6 误读提示

### 6.1 自动推断

范围（维护者定）：只对**整条文字恰好是词库词**的候选（≥2 字）做推断——`tiaoyan` 拼出的「调研」能标；
整句里嵌着的词（「我们去调研」）**本轮不标**，留待下轮（需引擎在全精确命中时也保留词边界）。

> 实测（Task 8）：自动推断只给引擎**已经出成整词候选**的词补提示，不召回。大词库下 `tiaoyan` 的候选里
> 根本没有「调研」，推断无从命中；误读主要靠 §6.2 的表（内置表已收「调研 tiao yan」）。

1. 按 `text` 查词→读音索引。拼音方案目前**没有**常驻的词→码反查（`ReverseIndex` 只供码表，
   `manager.rs:1352`；且只存码、不带权重）。维护者定：**按需建**——仅当「显示正确拼音」开且勾了误读来源、
   或生效模板引用了 `${correct_pinyin}` 时才建；实现第一步**先量常驻与峰值内存**（按本仓三档口径），
   超出预期回来与维护者商量再定方案。取权重可改索引格式或回正向词库查，由实现者按实测定。未就绪时静默不标。
2. 该词在词库里**有登记**，且**所有**登记读音都 ≠ 用户音节（经模糊/纠错对齐后的用户音节）⇒ 误读。
3. 选读音：取权重最高的登记读音；逐字比对，**只标读音不同的字**（`调(diao)研`）。
4. 不标：单字；字数 ≠ 音节数；简拼段 / 前缀补全段 / 末尾半截音节（沿用 `align_hints` 的 Prefix 判类与 D10）；
   该差异已能被当前开启的模糊规则解释（归 `fuzzy` 来源，不重复）。

⚠️ 现状缺口：全部精确命中时整句 `pieces`/`learn_code` 为 `None`，`fill_fuzzy_hints` 在
`pinyin/mod.rs:2468` 直接 `continue`——`tiaoyan` 正是这种情况，误读要另开路径，不能只靠 `learn_code`。

⚠️ 已知风险：词库漏收合法读音时会误标。缓解：误读表可写「白名单」条目（§6.2）把它压掉；
评测（§9）统计打对输入的误标率。

### 6.2 误读表

- 文件：内置一份 + 用户目录追加一份（具体路径走资源层约定，由实现者定；新增 `data/` 文件要同步
  `build_dev/data`）。
- 行格式（草案）：`文字<Tab>错误拼音<Tab>正确拼音`，如 `馄饨	hun dun	hun tun`；正确拼音留空 = 白名单
  （「这个读法不算错」，压掉自动推断的误标）。
- 匹配：**文字 + 错误拼音双重匹配**（与雾凇/万象一致），命中即按表提示；与自动推断冲突时表优先。
- 表条目**召回候选并提示**（维护者定）：用户打表里的错误拼音时，即使词库读不出这个词，也以**低权重**放进候选，
  带正确拼音提示（`莘(shen)莘(shen)`）——否则读错的人根本打不出这个词。召回受「误读」来源勾选控制
  （勾了才召回）；不占 D6 意义上的首选；属候选行为变更，须登记 changelog。白名单条目不召回。
- 内置数据：自行整理初版（数十到上百条常见误读）；参考雾凇（GPL-3，与本仓同协议）须逐条核对、
  在文件头注明来源；万象须先核协议再决定能否参考。

## 7 正确拼音提示（通用化后）

- 延续 fuzzy-pinyin-hint 的显示规则：正文内嵌 `字(拼音)`、半角括号、全拼小写、可带调、截断整段取舍、
  简繁/逆切分换算、Android 同步。
- 来源：`fuzzy`（原逻辑）、`misread`（§6）、`correction`（§5，纠错音节 ≠ 用户音节时内嵌正确拼音）。
- `scope = "first"`：在**所有勾选来源合并后**只标第一个带提示的候选（本设计的判断）。
- 模板变量 `${correct_pinyin}`：候选带**已勾选来源**的提示时有值（整词正确拼音、空格分隔），否则空串；
  **按来源勾选过滤**（维护者定），但**不受**「显示正确拼音」总开关与 `scope` 约束。

## 8 设置端（wind-setting）

`group = "input"` 新 section「纠错与提示」，草案：

| 行 | 类型 | 说明 |
|---|---|---|
| 拼写纠错 | 总开关 + 扩展设置对话框 | 对话框：§5.2 规则逐条（带冲突示例 hint）+ 自定义规则列表 |
| 纠错候选标记 | string | `enabled_when` 纠错开 |
| 显示正确拼音 | toggle | 出厂关 |
| 提示来源 | 三个勾选：模糊音 / 误读 / 拼写纠错 | 不随「显示正确拼音」置灰（来源同时作用于模板变量与误读召回） |
| 提示范围 | select 首个/全部 | `enabled_when` 显示正确拼音 |
| 提示带声调 | toggle | 不依赖「显示正确拼音」（同原设计） |
| 误读表 | 打开文件 / 对话框 | 形式由实现者按现有「注释词库」版式定 |

- 模糊音区删掉三项提示（`manifest::build_fuzzy_row` 的手写部分一并清）。
- label 新字过拼音检索表守卫；`mockdata`、`capabilities.snapshot.json`、`search/index.rs`、`wizard/model.rs` 同步。
- Android 自带 manifest 同步。

## 9 验收与评测门禁

- **评测门禁**（`pinyin_eval`，按本仓「排序调参走评测扫参」约定）：开纠错出厂规则后，打对的输入
  首页候选位次与关纠错时**一致**（纠错候选不挤进打对输入的首页前 N，N 由扫参定）；误读自动推断在打对输入上的
  误标率给出数字。
  ⚠️ 评测集**必须含简拼与混合简拼输入**（`xign`、`dnu`、`nage`、`x'ai` 一类）：纠错只收非法拼写，打对的全拼
  从结构上召回不到纠错候选，只用全拼集门禁恒空过。夹具缺这类输入要先补；补完后若 `gn_ng`/`on_ong` 过不了门禁，
  出厂降为关。
- 全拼 `gognping` → 候选含「\*公平」，编码栏 `gogn'ping`；勾纠错来源后显示「\*公(gong)平」。
- `gongping`（打对）→ 无标记、无提示，与关纠错时逐字节一致。
- `tiaoyan` → 开提示（误读来源）显示「调(diao)研」（靠内置误读表召回，大词库下自动推断不命中）；
  `diaoyan` 无提示；单字 `tiao` 不标。
- 误读表白名单条目能压掉对应误标；表条目在自动推断不命中时仍能提示。
- 误读召回：表登记 `莘莘 xin xin → shen shen` 时，`xinxin` 候选里出现「莘(shen)莘(shen)」且不在首选；
  取消勾选误读来源后不再出现。
- `${correct_pinyin}`：只勾模糊音来源时，误读候选的该变量为空。
- 模糊音回归：原 fuzzy-pinyin-hint §8 全部用例在新配置键下照过。
- 自定义规则：`custom=["ogn=ong"]` 在 `gn_ng=false` 时单独生效；非法规则 warn 不崩。
- 候选行为有变（纠错新增候选）⇒ 同分支在 `docs/architecture/candidate-behavior-changelog.md` 登记。
- 关闭全部（纠错关、提示关）时显示与现状逐字节一致（golden 不变）。

## 10 已定项（2026-10-23 维护者答复）

- 整句误读：只标整条是词库词的候选（§6.1）。
- 反查内存：按需建，先量内存再定（§6.1）。
- 误读表：召回并提示（§6.2）。
- `${correct_pinyin}`：按来源过滤（§7）。

## 11 非目标与另立项

- 邻键误触纠错（含双拼键对颠倒、Android 触摸坐标——引擎目前无坐标接口 `wind-mobile/src/lib.rs:492`）。
- 多打/漏打（编辑距离）。
- 纠错候选在编码栏标红/波浪线（讯飞式）。
- 码表方案的任何纠错与提示。

### 来源

- 雾凇：`rime_ice.schema.yaml`（algebra）、`lua/corrector.lua`、`cn_dicts/others.dict.yaml`、issue #431 / #1384
- 万象：`wanxiang_algebra.yaml`、`dicts/cuoyin.dict.yaml`、`docs/doc/super_comment.md`、issue #545
- librime：`src/rime/dict/corrector.cc`、`gear/script_translator.cc`、`algo/calculus.cc`、CHANGELOG、issue #1120
- libime：`pinyincorrectionprofile.cpp`、`pinyincontext.cpp`、`pinyindata.cpp`
- 搜狗帮助 pinyin.sogou.com/help.php?list=4&q=6、更新日志；QQ 拼音 skin.qq.pinyin.cn/help/pinyin05.html；百度更新日志
