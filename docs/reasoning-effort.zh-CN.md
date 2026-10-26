# Bot 思考强度

在 Bot 详情中选择「思考强度」，保存后下一次模型调用生效。每个 Bot 独立配置，委派不会覆盖其他 Bot 的选择。默认不发送额外思考参数，保留供应商原有行为。强度控制模型推理投入，与回复长度、工具权限和是否向用户提问分别管理；不是通过在提示词中要求输出思维过程实现。

可选档位：供应商默认、关闭、最低、低、中、高、超高、最高，分别保存为空、`none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max`。可通过 `bot.update` 的 `effort` 字段设置；空字符串或 null 恢复默认。设置只用于 Bot 正式任务及其工具循环，内部摘要、审批等辅助请求保留原有默认行为。

| 协议 | 请求行为 |
|---|---|
| OpenAI Chat Completions 兼容 | 发送 `reasoning_effort`，将输出上限通过 `max_completion_tokens` 传递，以包含推理消耗 |
| Anthropic Messages 兼容 | 关闭时发送 `thinking.type=disabled`；其他档位发送 `thinking.type=enabled`，预算依次为 1024、2048、4096、8192、12288、16384 tokens，并限制为输出上限减 1；输出上限不大于 1024 时拒绝启用 |

不是每个模型都支持全部档位或相同的思考方式。Anthropic 此实现使用显式预算，不宣称支持所有模型的 adaptive thinking 或原生 effort；仅支持其他思考协议的模型可先使用供应商默认。供应商拒绝时保留实际错误，不悄悄降档或声称已生效。

OpenAI 参数依据：[官方推理模型指南](https://developers.openai.com/api/docs/guides/reasoning)。验证使用本地参数和协议测试，不调用付费模型；真实模型的支持范围、质量及延迟仍需在具体供应商上验证。

这些设置和界面编译进服务，修改源码后需构建并重启；Bot 的选择本身保存后无需重启。
