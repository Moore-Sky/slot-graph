# Slot Graph 设计

**版本：0.3**

> 本文档描述 `slot-graph` 0.3 的使用方式和可观察语义。
> 内容围绕典型场景展开；代码是接近 Rust 的 API 伪码，最终 API 应保持同等表达能力和心智模型。

---

# 1. 一句话定位

`slot-graph` 是一个：

> **把运行时类型化 Slot 连接图编译成不可变执行 DAG，并以 push 方式驱动同步 / async 节点执行的轻量核心。**

典型用途：

- renderer 中的渲染步骤编排；
- RenderTarget / transient Buffer 等中间结果流动；
- 资源准备；
- 数据转换；
- 异步准备链；
- 小型执行流水线。

它不是：

- renderer 或 frame graph；
- resource manager；
- ECS scheduler；
- async runtime、executor 或 thread pool；
- 带持久化、重试和 UI 的 workflow 产品。

---

# 2. 使用流程

使用者只需要理解三个阶段：

```text
声明 Graph
    ↓
compile
    ↓
不可变 ExecutionGraphVersion
    ↓
一次或多次 GraphRun
```

对应操作：

```text
声明 Node / Slot
连接 Slot
选择 Active target
compile
传入本次运行数据
execute
读取报告和 target 输出
```

声明图可以继续编辑。已经编译出的版本保持不变，正在执行的 run 也不会被后续编辑影响。

---

# 3. 最小使用体验

```rust
let mut graph = Graph::<Local>::new();

let prepare = graph.add_async(
    "prepare",
    schema! { () -> ("texture": TextureHandle) },
    |_task, _inputs| async move {
        let texture = prepare_texture().await?;

        Ok(outputs! {
            "texture" => texture,
        })
    },
)?;

let draw = graph.add_sync(
    "draw",
    schema! { ("texture": TextureHandle) -> () },
    |_task, inputs| {
        let texture = inputs.required::<TextureHandle>("texture")?;

        draw_texture(texture.as_ref())?;

        Ok(outputs! {})
    },
)?;

graph.connect(
    prepare.output("texture"),
    draw.input("texture"),
)?;

graph.set_active(draw, true)?;

let version = graph.compile()?;

let report = version
    .execute(RunInputs::new())
    .await?;
```

使用者看到的是：

```text
prepare.texture
       ↓
draw.texture
```

Slot 连接本身就是依赖，不需要再单独写 `draw depends_on prepare`。

---

# 4. Node 和 Slot

每个 Node 有：

- graph 内唯一的名称；
- 稳定的 `NodeId`；
- 输入 Slot；
- 输出 Slot；
- 一个 sync 或 async task。

名称是查询别名，不是内部身份：

```rust
graph.set_active("preview", true)?;
graph.set_active(preview_node, true)?;
```

rename 不改变 `NodeId` 和已有连接：

```rust
graph.rename_node(preview_node, "preview_hdr")?;
```

Node 被删除后，旧 Node/Slot/Edge handle 不会错误指向后来复用的对象；再次使用时返回 stale-handle 错误。另一个 Graph 创建的 handle 返回 `ForeignHandle`。

## 4.1 Schema

```rust
schema! {
    (
        "color": RenderTargetHandle,
        "depth": Optional<RenderTargetHandle>,
        "lights": Optional<Many<LightBuffer>>,
    )
    ->
    (
        "output": RenderTargetHandle,
    )
}
```

输入简写：

| 写法 | 含义 |
|---|---|
| `T` | `Required + One<T>` |
| `Optional<T>` | `Optional + One<T>` |
| `Many<T>` | `Required + Many<T>` |
| `Optional<Many<T>>` | `Optional + Many<T>` |

输出在 0.3 中都是单值输出。一个输出供多个消费者使用，通过 fan-out 连接表达，不通过 `Many` output 表达。

每个 Node 的 input 和 output 分别具有非空、各自唯一的名称和 Slot identity。实际连接身份由 Node、方向和 Slot identity 共同确定；重复名称或 identity 是 `InvalidSchema`。

Schema macro 只是声明便利工具。核心能力必须也能通过普通 Schema API 使用；macro 不负责连接、compile 或执行。

---

# 5. 场景一：普通同步数据流

```text
Cull.visible → BuildCommands.visible
```

```rust
let cull = graph.add_sync(
    "cull",
    schema! {
        ("scene": SceneHandle)
        ->
        ("visible": VisibleSet)
    },
    |_task, inputs| {
        let scene = inputs.required::<SceneHandle>("scene")?;
        let visible = cull_scene(scene.as_ref())?;

        Ok(outputs! {
            "visible" => visible,
        })
    },
)?;

let build = graph.add_sync(
    "build_commands",
    schema! {
        ("visible": VisibleSet)
        ->
        ("commands": DrawCommands)
    },
    |_task, inputs| {
        let visible = inputs.required::<VisibleSet>("visible")?;

        Ok(outputs! {
            "commands" => build_commands(visible.as_ref())?,
        })
    },
)?;

graph.connect(
    cull.output("visible"),
    build.input("visible"),
)?;
```

执行语义：

```text
cull 成功
    ↓
visible 提交
    ↓
build_commands Ready
    ↓
build_commands 执行
```

---

# 6. 场景二：一个节点多个输出

```rust
let gbuffer = graph.add_sync(
    "gbuffer",
    schema! {
        ("scene": SceneHandle)
        ->
        (
            "color": RenderTargetHandle,
            "depth": RenderTargetHandle,
            "normal": RenderTargetHandle,
        )
    },
    |_task, inputs| {
        let scene = inputs.required::<SceneHandle>("scene")?;
        let (color, depth, normal) = render_gbuffer(scene.as_ref())?;

        Ok(outputs! {
            "color" => color,
            "depth" => depth,
            "normal" => normal,
        })
    },
)?;
```

如果三个输出都连接到 Lighting：

```text
gbuffer.color  → lighting.color
gbuffer.depth  → lighting.depth
gbuffer.normal → lighting.normal
```

逻辑上有三条 Slot edge，但只有一个节点依赖：

```text
GBuffer → Lighting
```

GBuffer 成功后一次性提交全部输出，Lighting 只被解锁一次。

## 6.1 输出原子提交

一个成功 task 必须返回 Schema 声明的全部输出，每项恰好一次。以下情况都使整个节点失败，并且一个输出也不提交：

- 缺少声明输出；
- 多出未知输出；
- 重复输出；
- 输出类型错误；
- task 返回错误或 panic。

task 完成与取消发生竞态时，输出仍以整个节点为单位提交。task 返回（async task 即 Future Ready）并通过完整性校验后，运行时必须在“取消生效”和“提交全部输出”之间确定唯一顺序：

```text
task 返回 / Future Ready
    ↓
校验完整 outputs
    ↓
原子决定 cancel 与 commit 的顺序
    ├─ cancel 先发生 → 丢弃全部 outputs，节点为 Cancelled
    └─ commit 先发生 → 提交全部 outputs，节点为 Succeeded
```

一旦 commit 先完成，之后的取消不会撤销这些 outputs，也不会把该节点从 `Succeeded` 改为 `Cancelled`。任何情况下，下游都看不到部分 outputs。

原子性只保护 SlotGraph 中的输出可见性。task 已执行的 I/O、资源分配、GPU command record 或外部状态修改不会自动回滚。

---

# 7. 场景三：Async 节点

```rust
let load = graph.add_async(
    "load_data",
    schema! {
        ("asset": AssetId)
        ->
        ("data": DecodedData)
    },
    |_task, inputs| async move {
        let asset = inputs.required::<AssetId>("asset")?;

        let bytes = load_bytes(asset.as_ref()).await?;
        let data = decode(bytes).await?;

        Ok(outputs! {
            "data" => data,
        })
    },
)?;
```

`NodeInputs` 持有本次 task 所需输入的共享所有权句柄，因此可以安全 move 进 Future，并跨越 `.await`。访问方式为：

```rust
inputs.required::<T>("input")? // Shared<T>
inputs.optional::<T>("input")? // Option<Shared<T>>
inputs.many::<T>("input")?     // Vec<Shared<T>>
```

为保持示例简洁，文中把实际的 `Shared<T, Mode>` 简写为 `Shared<T>`。它只共享所有权，不复制 `T`；Local 模式支持本地值，Send 模式支持满足其线程安全约束的值。

`Shared<T, Mode>` 是公开但不透明的 Slot value wrapper。使用者可以依赖 `Clone`、`as_ref()` / `Deref<Target = T>` 的只读访问，以及 Mode 对应的线程能力；clone 只增加对同一个值的持有，不要求也不会复制 `T`。公开 API 不提供可变引用、引用计数查询或底层容器转换，使用者不能依赖其具体共享实现、地址身份或引用计数。

输入读取和报告输出始终使用该 wrapper。需要可变共享状态时，由 `T` 自己通过 handle、cell 或 lock 等宿主类型表达；`Shared` 不替代 AssetManager、GPU submission 或 fence 的资源生命周期。

`slot-graph` 负责轮询节点 Future 和传播完成事件，但不选择 Tokio、smol 或其他 runtime。

---

# 8. 场景四：Optional 和 Many

Presence 与 Cardinality 是两个独立维度：

| 声明 | 允许来源数 |
|---|---:|
| Required + One | 恰好 1 |
| Optional + One | 0 或 1 |
| Required + Many | 至少 1 |
| Optional + Many | 0 或多个 |

## 8.1 Optional

```rust
let compose = graph.add_sync(
    "compose",
    schema! {
        (
            "image": Image,
            "mask": Optional<Mask>,
        )
        ->
        ("result": Image)
    },
    |_task, inputs| {
        let image = inputs.required::<Image>("image")?;
        let mask = inputs.optional::<Mask>("mask")?;

        Ok(outputs! {
            "result" => compose_image(
                image.as_ref(),
                mask.as_deref(),
            )?,
        })
    },
)?;
```

Optional 只表示“可以没有来源”。如果 Optional input 已连接 producer，它就是普通依赖，consumer 会等待 producer 成功。

Optional 不表示 lazy，也不表示 producer 可以尚未完成。

## 8.2 Many

```rust
let merge = graph.add_sync(
    "merge",
    schema! {
        ("items": Many<Item>)
        ->
        ("merged": ItemSet)
    },
    |_task, inputs| {
        let items = inputs.many::<Item>("items")?;

        Ok(outputs! {
            "merged" => merge_items(&items)?,
        })
    },
)?;

graph.connect(a.output("item"), merge.input("items"))?;
graph.connect(b.output("item"), merge.input("items"))?;
graph.connect(c.output("item"), merge.input("items"))?;
```

0.3 的 readiness 固定为 All：Many 会等待全部已连接来源成功。

Many 的值顺序按连接建立顺序固定，不按 producer 完成顺序。断开后重新连接的 edge 排到现存连接之后。

---

# 9. 场景五：每次运行传入外部数据

相机、frame index、请求参数等每次运行都可能不同，不应通过修改 task 或重新 compile 传入。

先把没有 producer 的输入显式暴露为运行入口：

```rust
let cull = graph.add_sync(
    "cull",
    schema! {
        ("scene": SceneHandle)
        ->
        ("visible": VisibleSet)
    },
    |_task, inputs| {
        let scene = inputs.required::<SceneHandle>("scene")?;

        Ok(outputs! {
            "visible" => cull_scene(scene.as_ref())?,
        })
    },
)?;

let scene_input = graph.expose_input::<SceneHandle>(
    cull.input("scene"),
)?;

graph.set_active(cull, true)?;

let version = graph.compile()?;
```

每次运行传入不同值：

```rust
let mut inputs = RunInputs::new();
inputs.insert(scene_input, current_scene)?;

let report = version.execute(inputs).await?;
```

规则：

- exposed input 是由 `RunInputs` 提供的 run-scoped Slot source；启动校验成功后，它与普通 producer 已提交的 output 使用相同的输入可见性和 readiness 规则；
- exposed input 不能同时连接普通 producer；
- Required 外部输入缺失时，任何 task 都不会启动；
- Optional 外部输入可以不传；
- Many 外部输入使用 `extend` 一次传入有序集合；
- 未知、重复、数量错误或类型错误在 run 启动前返回结构化错误；
- 已编译版本保存自己的入口定义，后续编辑声明图不会改变旧版本接受的 RunInputs。

对 task 而言，外部入口和上游 output 都是 input 的值来源；区别只在于外部值在 `start` 前提供，上游值在 producer 成功后提供。一个 selected node 只有在每个 input 都已解析时才会启动：

- 已连接的 One / Many 等待其全部 producer 成功；任一 producer Failed、Cancelled 或 Blocked 时，consumer 为 Blocked，不以部分值启动；
- exposed input 通过启动校验后视为已经提供，不额外执行节点；
- 未连接且未 expose 的 Optional One / Many 立即解析为 `None` / 空集合；Required input 则不能通过 compile；
- exposed input 与普通 producer 互斥，因此 0.3 不把同一个 Many input 的值拆成一部分来自 `RunInputs`、另一部分来自 graph。

`RunInputs` 校验失败是 `StartError`，不会产生已经部分启动的 GraphRun。它只负责给本次 run 提供 Slot value，不引入另一套节点调度或完成语义。

宿主也可以让零输入 source task 从捕获的 `Arc` / `Rc` 服务读取状态，但多个重叠 run 的快照一致性由宿主负责。

---

# 10. 场景六：读取 Active target 输出

Active target 的成功输出由 `RunReport` 保留：

```rust
let final_output = graph.output::<RenderTargetHandle>(
    post,
    "final",
)?;

graph.set_active(post, true)?;

let version = graph.compile()?;
let mut report = version.execute(RunInputs::new()).await?;

let final_rt = report.take_output::<RenderTargetHandle>(
    final_output,
)?;
```

`node.output("name")` 是构图时的名称 selector；需要长期保存并从报告读取时，使用 `graph.output::<T>(node, "name")?` 取得已验证的 typed output handle。

`report.output(handle)` 借用查看报告中的值；`report.take_output(handle)` 从报告移出 `Shared<T>`。后者适合 render target 等所有权资源：移出后，其生命周期由调用者控制；再次读取同一输出会返回 `OutputTaken`。

`RunReport` 对尚未取出的 target output 持有 strong Slot value。因此，只要报告仍存活，对应的 transient resource 就可能无法返回资源池；不再需要报告时应及时 drop，仍需使用的资源则通过 `take_output` 明确接管。

0.3 的运行工作集会保留普通中间 Slot value，直到 run 到达终态；成功 Active target 的输出随后由报告继续持有。0.3 不做 last-consumer early release，精确 Drop 时机也不是公开契约。未来允许在最后一个 consumer 完成后提前释放中间值，这属于内存优化，不改变图的可观察语义。

报告默认只保留成功 Active target 的输出：

- 非 target 输出返回 `NotCollected`；
- Failed、Cancelled 或 Blocked target 没有新输出；
- 如果多个 Active target 中一部分成功、另一部分失败，成功 target 的输出仍可从失败报告中读取。

报告始终按产生它的编译版本验证 output handle，不查询后来编辑过的声明图。因此替换 Schema 后，v1 的报告仍接受 v1 的旧 handle；其他 Graph 的 handle 返回 `ForeignHandle`，不属于该版本 Schema 的 handle 返回 `StaleSlotHandle`，非 target output 返回 `NotCollected`。

如果 Active target 只产生外部副作用，也可以声明零输出。

---

# 11. 场景七：长期资源与 transient 资源

SlotGraph 只表达本次执行的数据依赖，不接管宿主资源系统。

## 11.1 长期 Texture

普通图片 Texture 通常由 AssetManager / ResourceManager 长期持有：

```text
AssetManager
    └── Texture #17

Material
    └── TextureHandle(17)
```

节点按需读取：

```rust
let material = inputs.required::<MaterialHandle>("material")?;
let texture_handle = material_system.albedo(material.as_ref())?;
let texture = resources.get_texture(texture_handle)?;

draw(texture)?;
```

不需要为了长期资源构造：

```text
TextureNode → EveryRenderNode
```

如果 Texture 确实是本次执行产生的结果，则可以正常进入 Slot：

```text
Decode → UploadTexture → UseTexture
```

判断标准不是“它是不是 Texture”，而是“它是不是这次执行中的数据依赖”。

## 11.2 Transient RenderTarget

```text
GBuffer
   ├── ColorRT ───────┐
   ├── NormalRT ──────┼→ Lighting → HDR RT → PostProcess
   └── DepthRT ───────┘
```

RenderTargetHandle 可以通过 Slot 流动，但真实资源生命周期由 renderer 管理：

```text
TransientResourcePool.alloc
    ↓
SlotGraph 传递 Handle
    ↓
record / submit
    ↓
frame-in-flight 持有
    ↓
GPU fence complete
    ↓
归还 pool
```

关键原则：

> **CPU GraphRun 结束不等于 GPU 已经不再使用资源。**

GraphRun drop 只释放它持有的 Slot value / Handle，不负责销毁 GPU resource、插入 barrier 或等待 fence。

---

# 12. 场景八：一个节点按需读取少量长期资源

一个节点理论上可能使用十张 Texture，本次因为 material flags、LOD 或画质只使用两张时，不应把十张都建成 Slot dependency。

```rust
let material = inputs.required::<MaterialHandle>("material")?;

if material.use_albedo {
    let texture = resources.get_texture(material.albedo)?;
    use_texture(texture);
}

if material.use_normal {
    let texture = resources.get_texture(material.normal)?;
    use_texture(texture);
}
```

边界是：

```text
本次执行的数据流      → Slot push
长期 Asset 查询       → node 内按需 pull
```

---

# 13. 场景九：Active target

Active 表示“这个版本请求执行的目标”，编译器自动加入目标的全部上游依赖。

```rust
graph.set_active(main_render, true)?;
graph.set_active(debug_export, export_enabled)?;

let version = graph.compile()?;
```

多个 target 的依赖会合并：

```text
        A
       / \
      B   C
      |   |
      D   E

Active: D, E
执行:   A, B, C, D, E
```

同一 Node 在一次 GraphRun 中最多执行一次。

## 13.1 Active 不是 enabled 开关

如果 `post` 是 Active 且存在：

```text
ssao → post
```

那么 `ssao` 会作为 `post` 的依赖执行，即使 `ssao` 自己没有被设为 Active。

画质切换应使用不同 target 或修改连接。例如声明两条 pipeline：

```text
Lighting ───────────────→ LowPost
    └→ SSAO → Bloom ────→ HighPost
```

低画质：

```rust
graph.set_active(low_post, true)?;
graph.set_active(high_post, false)?;

let low = graph.compile()?;
```

高画质：

```rust
graph.set_active(low_post, false)?;
graph.set_active(high_post, true)?;

let high = graph.compile()?;
```

Active 通常对应画质、机器能力、功能模式、preview/export 等低频变化，不用于每帧切换大量节点。

---

# 14. 场景十：编辑图并发布新版本

声明图支持：

```rust
graph.add_sync(...)
graph.add_async(...)
graph.remove_node(...)
graph.rename_node(...)
graph.replace_task(...)
graph.replace_schema(...)

graph.connect(...)
graph.disconnect(edge_id)
graph.reconnect(edge_id, new_output)

graph.set_active(...)
```

`compile()` 每次完整编译当前声明，返回新的不可变版本：

```rust
let v1 = graph.compile()?;
let run_v1 = v1.start(RunInputs::new())?;

let edge = graph.reconnect(
    edge,
    producer_b.output("texture"),
)?;

let v2 = graph.compile()?;
```

语义：

```text
run(v1) 继续使用 v1
新 run 可以使用 v2
```

`compile` 不自动发布 current version。宿主决定何时替换当前版本：

```rust
let v2 = graph.compile()?;
current.store(Arc::new(v2));
```

compile 失败时不会产生新版本，也不会修改宿主已经发布的版本。

编辑操作自身是原子的：

- `connect` 成功后返回 `EdgeId`；重复 edge 或超过 One 基数会失败；
- `disconnect(edge_id)` 精确删除一条 edge；
- `reconnect` 对新 source 执行与 `connect` 相同的方向、类型、重复 edge 和基数检查，再替换旧 edge；失败时旧连接不变；
- `remove_node` 同时删除它的 incident edges 和 Active 标记；
- `replace_task` 保持 Schema 和 edge；
- `replace_schema` 按下面的规则保留仍兼容的连接，并报告被移除的 edge 和 exposed input。

`replace_schema` 成功后保持 NodeId，但此前从声明图取得的 Slot handle 全部 stale；即使对应 edge 被保留，使用者也必须按新 Schema 重新取得 input / output handle。Slot 对应关系只由相同方向、相同 Slot identity 和精确类型确定，不依赖名称或声明位置。

通过上述对应关系仍然有效的 edge 会保留原 `EdgeId` 和连接顺序。没有对应 Slot 的 edge 从声明图移除并进入返回报告，其 `EdgeId` 随即 stale。Presence 或 `auto_collect` 的变化本身不删除兼容 edge：新的 Required 约束由 compile 检查，`auto_collect` 只影响之后的自动连接。

所有待保留 edge 必须能同时满足新 Schema 的基数和 binding 约束。如果 `Many → One` 后存在多个候选 edge，或出现其他无法整体保留的冲突，`replace_schema` 原子失败，旧 Schema、Slot handle、edge 和 exposed input 全部不变；操作不会按连接顺序静默挑选一个子集。

exposed input 只有在 Slot identity、精确类型、Presence 和 Cardinality 都未变化时才保留；否则从声明图移除并进入返回报告。旧编译版本仍接受自己保存的旧输入 key，新版本不接受已经被移除的 key。

0.3 不做增量编译；每个版本都来自一次完整 compile。

---

# 15. Compile 的可观察语义

`compile()` 负责：

- 计算 Active target 的反向依赖闭包；
- 合并多个 target 的重叠依赖；
- 检查 selected subgraph 的 Required input；
- 检查类型、基数和 binding；
- 检测 selected subgraph 的环；
- 生成不可变执行版本。

未被选中的备用分支可以暂时缺 Required input 或含环，不阻止当前 target 编译；当该分支成为 Active target 时，compile 会报告问题。

以下问题在编辑操作当场拒绝，不延迟到 compile：

- foreign / stale handle；
- Slot 方向错误；
- 精确类型不匹配；
- One input 来源过多；
- 重复 edge；
- exposed input 与普通 producer 冲突。

没有任何 Active target 时，compile 返回 `NoActiveTarget`。

---

# 16. 场景十一：Local 与 Send

Local 模式允许本地值和 `!Send` Future：

```rust
let mut graph = Graph::<Local>::new();

let state = Rc::clone(&state);
graph.add_async(
    "local_task",
    schema! { () -> () },
    move |_task, _inputs| {
        let state = Rc::clone(&state);

        async move {
            state.borrow_mut().prepare().await?;
            Ok(outputs! {})
        }
    },
)?;
```

Send 模式要求 task、Future 和 Slot value 满足跨线程约束：

```rust
let mut graph = Graph::<SendMode>::new();

let resources = Arc::clone(&resources);
graph.add_async(
    "prepare_texture",
    schema! {
        ("asset": AssetId)
        ->
        ("texture": TextureHandle)
    },
    move |_task, inputs| {
        let resources = Arc::clone(&resources);

        async move {
            let asset = inputs.required::<AssetId>("asset")?;
            let texture = resources
                .prepare_texture(asset.as_ref())
                .await?;

            Ok(outputs! {
                "texture" => texture,
            })
        }
    },
)?;
```

同一个不可变版本可以重复启动多个独立 run。Send version 可以被宿主共享，并从不同线程启动 run；同一个 GraphRun 不能被并发 poll。

task factory 必须可重复调用。跨 run 的可变状态由使用者显式放入 `RefCell`、`Mutex` 或宿主资源系统。

## 16.1 不拥有 Executor

`version.execute(...).await` 返回一个普通 Future。核心：

- inline 执行短小 sync task；
- 同时保持多个 pending async task；
- 根据完成结果解锁后继；
- 不创建线程；
- 不 spawn task；
- 不选择 Tokio、smol、async-std 或 rayon；
- 不保证独立 sync task 的 CPU 并行。

需要线程池工作的 node 可以通过捕获的宿主服务提交工作，并返回等待结果的 Future。

无依赖节点的完成和外部副作用顺序不保证稳定。需要顺序时必须显式连边。

---

# 17. 场景十二：高频重复执行

普通接口优先简单：

```rust
let report = version.execute(inputs).await?;
```

frame loop 需要复用运行缓冲时，使用独占 runner：

```rust
let mut runner = version.runner();

loop {
    let run = runner.start(next_frame_inputs())?;
    let control = run.control();
    register_frame_cancel_handle(control);
    let report = run.await?;

    present(report)?;
}
```

`runner.start(inputs)` 返回同时实现 Future 和 `control()` 的 `RunnerRun<'_, Mode>`；它在存活期间独占借用 runner。`runner.execute(inputs).await` 是不需要 control 时的简写。一个 runner 同时只运行一次；需要多个 frame-in-flight 时，创建多个 runner。运行完成、取消或被 drop 后，runner 才能开始下一次运行。

runner 只复用 CPU 侧 GraphRun 存储，不改变 Slot value 和外部 GPU resource 的生命周期语义。

---

# 18. 场景十三：失败

```text
A Failed
    ↓
B Blocked
    ↓
C Blocked

D → E 仍可完成
```

GraphRun 不在第一个错误处立即丢弃其他独立分支。它完成所有仍可推进的分支后返回报告。

```rust
match version.execute(inputs).await {
    Ok(report) => {
        // 全部 selected node 成功
    }
    Err(ExecuteError::Failed(report)) => {
        for failure in report.failures() {
            log_failure(failure);
        }
    }
    Err(ExecuteError::Cancelled(report)) => {
        log_cancelled(report);
    }
    Err(ExecuteError::Start(error)) => {
        // RunInputs 在任何 task 启动前校验失败
    }
}
```

`RunReport` 至少可以查询：

- version / run identity；
- selected Node 的最终状态；
- 全部 task failure；
- Blocked 的直接原因；
- 成功 Active target 的输出。

节点状态：

```rust
enum NodeStatus {
    Pending,
    Ready,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Blocked,
}
```

用户错误通过 `NodeError::User` 返回。unwind panic 转换为该节点失败；`panic = abort` 无法转换成运行报告。

核心不提供 retry、fallback、rollback 或持久化恢复。

---

# 19. 场景十四：取消

需要外部取消时先取得 control，再把 run 交给宿主驱动。下面是 SendMode 场景：

```rust
let run = version.start(inputs)?;
let control = run.control();

host.spawn(async move {
    run.await
});

if user_cancelled {
    control.cancel();
}
```

`run.control()` 返回拥有独立共享状态、可 Clone 的 `RunControl<Mode>`，不借用 run。`RunControl<Local>` 只能在本地线程使用；`RunControl<SendMode>` 可以跨线程请求取消。Local run 使用 `spawn_local` 或由当前线程直接 poll。

协作取消：

- 不再启动新节点；
- 通知正在运行的 async task；
- task 完成与取消竞态时，按 §6.1 的单一顺序点裁决：取消先发生则丢弃全部 outputs，commit 先发生则保留全部 outputs；
- sync task 不能强制抢占；
- 不响应取消的 Future 可能一直 Pending。

因此，取消不回滚已经提交的 Slot outputs，也不回滚 task 已产生的外部副作用。取消后的报告仍可读取或取出取消前已经成功提交的 Active target 输出。

task 可以主动检查：

```rust
task.cancellation().is_cancelled()
task.cancellation().checkpoint()?
task.cancellation().cancelled().await
```

需要立即放弃时：

```rust
control.abort();
```

abort 会唤醒 GraphRun；GraphRun 在下一次 poll 前丢弃剩余 Future，不再调用用户 task，并返回 Cancelled。drop GraphRun 也会丢弃它持有的 Future，但不会生成报告，也不会回滚外部副作用。

`abort` 与节点 commit 的竞态同样遵循 §6.1：abort 先发生时丢弃该节点尚未提交的全部 outputs；commit 先发生时保留已经提交的 outputs。abort 不撤销此前成功节点的状态或 Active target 输出。

---

# 20. 场景十五：自动连接

显式连接始终是权威操作：

```rust
graph.connect(
    gbuffer.output("color"),
    lighting.input("color"),
)?;
```

Schema 明确时可以使用 convenience：

```rust
let report = graph.connect_nodes(gbuffer, lighting)?;
```

自动连接规则：

1. 只考虑 source outputs → target inputs；
2. 类型必须精确兼容；
3. 对 One input，优先选择 `exact name + type`；没有该候选时，才选择唯一的同类型候选；
4. 0.3 没有跨 Node 的 semantic key，也不比较 source 与 target 的 Slot identity；Slot identity 只用于标识各自 Node 内的 Slot；
5. 已连接的 One 不参与；
6. Many 只有声明 `auto_collect` 时才自动收集；它收集所有尚未与该 input 建立 edge、类型精确匹配的 source outputs，不套用 One 的 exact-name 优先规则；
7. 已存在的 edge 不重复建立；
8. 歧义直接报错；
9. 不使用模糊字符串或 HashMap iteration order 决胜；
10. 先只读形成完整计划，再按 target input 的 Schema 声明顺序、source output 的 Schema 声明顺序建立 edge；该顺序也是 report 顺序和 Many 连接顺序；
11. Many 收集前排除已存在的 `(output, input)` edge；
12. 整个操作原子成功或失败。

未匹配的 Required / Optional input 通过 report 返回。Required 是否最终满足仍由 compile 检查。

---

# 21. 确定性

以下顺序必须稳定：

- Many input 中的来源顺序；
- compile diagnostics；
- edge 和 Schema enumeration；
- RunReport 中的 Node 和错误顺序；
- 可查询的 execution metadata。

这些顺序不依赖 HashMap iteration 或 async 完成时序。

独立节点的实际完成先后不属于确定性保证。task 不应依赖没有 edge 约束的副作用顺序。

---

# 22. 公开 API 方向

0.3 的公开 API 围绕使用动作组织：

```rust
Graph::<Local>::new()
Graph::<SendMode>::new()

graph.add_sync(...)
graph.add_async(...)

graph.remove_node(...)
graph.rename_node(...)
graph.replace_task(...)
graph.replace_schema(...)

graph.connect(...)
graph.disconnect(edge_id)
graph.reconnect(edge_id, new_output)
graph.connect_nodes(...)

graph.expose_input(...)
graph.output::<T>(node, name)
graph.set_active(...)
graph.compile()

version.start(run_inputs)
version.execute(run_inputs).await
version.runner()
runner.start(run_inputs)
runner.execute(run_inputs).await

run.control()
control.cancel()
control.abort()

report.status(node)
report.failures()
report.output(output_slot)       // &Shared<T>，借用查看
report.take_output(output_slot)  // Shared<T>，从报告移出
```

---

# 23. 错误模型

错误按发生阶段区分。

## 23.1 Build / Edit

```text
DuplicateNodeName
InvalidNodeName
InvalidSchema
UnknownNodeName
UnknownSlotName
StaleNodeId
StaleSlotHandle
StaleEdgeId
ForeignHandle
WrongDirection
TypeMismatch
CardinalityOverflow
DuplicateEdge
InputSourceConflict
AmbiguousAutoMatch
```

## 23.2 Compile

```text
MissingRequiredInput
NoActiveTarget
CycleDetected
InvalidBinding
```

## 23.3 Start

```text
MissingRunInput
DuplicateRunInput
UnexpectedRunInput
RunInputCardinality
RunInputTypeMismatch
```

## 23.4 Node

```text
User
Panic
InvalidOutputs
InternalInvariantViolation
```

## 23.5 Report

```text
ForeignHandle
StaleSlotHandle
NotCollected
OutputUnavailable
OutputTaken
```

`OutputUnavailable` 表示对应 Active target 未成功；`OutputTaken` 表示值已通过 `take_output` 移出。报告查询失败不改变报告中其他输出的所有权或可读性。

## 23.6 Execute

```text
Start(StartError)
Failed(RunReport)
Cancelled(RunReport)
```

公开错误保持结构化并携带适用的 Node / Slot / Edge 和名称上下文。核心不用单一 `anyhow::Error` 代替库错误；用户 task 可以把自己的 source error 放入 `NodeError::User`。

公开 error、outcome 和 `NodeStatus` 从首次发布起保留扩展空间；0.3.x 不改变 Active、Many 顺序、原子输出和取消等可观察语义。需要破坏这些语义的变更进入 0.4。`Display` 文本不作为机器解析格式。

---

# 24. 性能边界

典型规模约为 100 个节点，目标环境可能以 60 / 90 / 120 FPS 重复运行。

原则：

> **compile 可以相对复杂；run 热路径必须简单。**

运行阶段应避免：

- 字符串和 HashMap lookup；
- 动态图遍历；
- sync node 的 Future boxing；
- 每次 poll 重复分配 async Future；
- 重复增长临时容器；
- 因 fan-out 复制实际值。

0.3 不做 last-consumer early release，因此峰值内存包括 run 到达终态前仍保留的普通中间 Slot value。benchmark 应记录这一基线；未来可以提前释放已无消费者的中间值，而不改变公开执行语义。

benchmark 至少覆盖：

- 100 / 1,000 / 10,000 node；
- chain、fan-out、fan-in Many；
- Active closure；
- sync、async 和 mixed graph；
- fresh execute 与 runner reuse；
- build、compile、run、reset、内存峰值和 allocations/run。

必须有 100-node 空任务 frame-loop benchmark，用于观察框架自身开销。10,000-node 用于发现增长趋势，不代表典型 renderer 规模。

Benchmark 使用确定性的 synthetic workload，不混入真实 GPU、文件 I/O 或网络。结果记录 commit、rustc、release profile、机器环境、ns/run、allocations/run 和峰值/保留内存；100-node 用作版本基线，1,000 / 10,000-node 用作增长趋势检查。

---

# 25. 0.3 必测使用场景

## 25.1 Graph 和连接

- 普通 `N1 → N2`；
- 多输入、多输出和 fan-out；
- 同一 node pair 多条 Slot edge，consumer 只解锁一次；
- Required / Optional 与 One / Many 全组合；
- Many 稳定顺序；
- 自动连接的 `exact name + type` 优先、unique type fallback、跨 Node Slot identity 不参与匹配、歧义和原子失败；
- `Many + auto_collect` 收集全部同类型候选、跳过已有 edge，并保持 source Schema 声明顺序；
- cycle、类型错误、基数错误和缺 Required input。

## 25.2 Run

- sync → async → sync；
- 多个 async task 同时 Pending；
- Pending 不提前解锁后继；
- outputs 完整原子提交；
- 同一 Node 每个 run 最多执行一次；
- 外部 RunInputs 的 Required / Optional / Many；
- Active target 输出的借用读取、`take_output` 所有权移出、重复 take 和报告持有期间的 strong value 生命周期；
- report access 区分 foreign handle、版本不匹配、非 target、target 未成功和已经 taken；v1 report 在 Schema 替换后仍接受 v1 handle；
- 同一 version 重复和并发运行互不影响；
- runner 完成、取消和 drop 后复用。

## 25.3 Failure / Cancellation

- task failure 阻塞下游；
- 独立分支继续；
- 多个失败全部进入报告；
- panic、错误 outputs 和取消不泄漏部分值；
- cooperative cancel；
- cancel / abort 先于 commit 与 commit 先于 cancel / abort 的竞态结果，且后者保留已提交的 Active target 输出；
- abort 和 drop pending Future；
- 重复、虚假和过期 wake 不重复执行节点。

## 25.4 Edit / Version

- add/remove/rename；
- replace task；
- replace schema 在 Slot 改名或重排但 identity / type 不变时保留 edge、`EdgeId` 和 Many 顺序，同时使旧 Slot handle stale；
- replace schema 在 identity / type / direction 不兼容时只移除并报告受影响 edge，且被移除的 `EdgeId` stale；
- `Many → One` 的零条、一条和多条候选 edge；多条时替换原子失败并完整保留旧图，`One → Many` 保留兼容 edge；
- Presence / `auto_collect` 变化不删除兼容 edge，并分别由后续 compile / auto-connect 验证新行为；
- exposed input 的完整契约未变时保留，identity / type / Presence / Cardinality 变化时移除；旧 version 仍接受旧 key，新 version 拒绝已移除 key；
- stale / foreign handle；
- connect/disconnect/reconnect；
- Active target 变化；
- inactive 不完整分支；
- compile failure 不影响旧版本；
- old/new version 同时运行。

## 25.5 类型和宿主

- Local 接受 `Rc` 和 `!Send` Future；
- Send 拒绝 `!Send` value、Future 和 task capture；
- 手写 poll loop 和至少一个测试 runtime 得到相同结果；
- 核心依赖不包含 async runtime 或 executor。

---

# 26. 工程约束

```text
Rust edition 2021
MSRV Rust 1.71
std required
```

Local / Send 是类型层模式，不是 Cargo feature。

CI 至少覆盖：

- fmt；
- clippy all targets / features；
- stable tests、doctests 和 examples；
- Rust 1.71 tests；
- rustdoc warnings as errors。

Cargo.lock 纳入仓库，MSRV CI 使用 lockfile。引入或升级 core、test、example、benchmark 依赖时，都必须重新在 Rust 1.71 上解析并测试；不兼容时收紧依赖版本范围或在 breaking release 调整 MSRV。

`Cargo.toml`、release note 和本文版本必须一致。如果 0.2 已公开发布，0.3 release note 必须列出 breaking changes；如果 0.2 只有 scaffold，则声明 0.3 是首个可用 API。

---

# 27. 0.3 明确不解决的问题

- Lazy Slot；
- `ReadyPolicy::Any / Min / Custom`；
- streaming、channel 和 backpressure；
- loop、feedback edge 和持续 workflow；
- retry、fallback、rollback、checkpoint / resume；
- 增量编译；
- 增量重跑和缓存；
- 内置 executor、线程池和 CPU 并行 scheduler；
- ECS Read / Write 冲突分析；
- GPU barrier、queue、fence、aliasing 和 residency；
- 跨进程 / 跨语言稳定类型系统；
- graph / task / value 序列化；
- UI、插件市场、远程 worker 和持久化历史；
- Slot value SBO、inline Any 和自研 allocator。

---

# 28. 0.3 Definition of Done

0.3 完成时，使用者应能：

1. 用简短 API 声明和连接 sync / async 节点；
2. 表达多输入、多输出、Optional、Many 和 fan-out；
3. 显式传入每次运行数据并读取 target 输出；
4. 用 Active target 选择需要执行的子图；
5. 在旧版本运行时编辑声明图并编译新版本；
6. 在 Local 中使用 `!Send`，在 Send 中获得明确的跨线程约束；
7. 使用任意符合 Rust Future 合约的宿主驱动运行；
8. 从结构化报告中定位失败、Blocked、Cancelled 和成功输出；
9. 取消或 abort 运行而不产生部分 Slot 输出；
10. 在 frame loop 中通过 runner 复用运行存储；
11. 让长期 Asset 留在 ResourceManager，让本次执行结果通过 Slot 流动；
12. 让 CPU GraphRun 与 GPU resource lifetime 保持清晰边界。

以上每项都必须有可运行 example 或自动化测试；第 25 节测试矩阵、MSRV CI 和第 24 节 benchmark 同时完成后，0.3 才达到发布条件。

---

# 29. 设计原则

> **先从使用场景和 API 出发。**

> **Slot 表达本次执行的数据流，不承担所有资源关系。**

> **长期 Asset 通常由 ResourceManager 持有；执行中间结果适合走 Slot。**

> **push 数据，节点成功后原子提交全部 outputs。**

> **一个 Node 在一次 GraphRun 中最多执行一次。**

> **编译版本不可变，编辑和发布由宿主显式控制。**

> **slot-graph 只理解 Future，不拥有线程和 executor。**

> **先保持 API 简单和语义可验证，再依据真实 workload 优化。**
