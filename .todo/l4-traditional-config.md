# Task 0.4: L4 Traditional Configuration Strategy

**Status:** Needs Clarification (Phase II)

**User Input:** L4 保留传统配置（上古时代的传统配置），不强制要求 Flow，后面的层（L4+, L7）不支持

**Blocker:** Need to clarify what "traditional configuration" means

## Current Understanding

- L4+ and L7 currently use Flow-based configuration only
- L4 historically had some "traditional configuration" (not Flow-based?)
- User wants to keep L4 traditional config as an option

## Questions to Clarify

1. 什么是"传统配置"？是指类似 Nginx 的 server/location block？还是其他格式？
2. 当前 L4 的代码中有传统配置的实现吗？在哪里？
3. "保留"是指：
   - 保持现状（不删除现有代码）？
   - 还是需要重新设计一个传统配置格式？
4. 传统配置和 Flow 配置是否可以共存？还是互斥？

## Example of What Might Be "Traditional Config"

```yaml
# Traditional (hypothetical)
listeners:
  - port: 80
    protocol: tcp
    action: proxy
    target: "192.168.1.10:8080"

# vs. Flow-based (current)
port_80:
  listen: "0.0.0.0:80"
  protocol: tcp
  flow:
    internal.transport.proxy.transparent:
      input:
        target.ip: "192.168.1.10"
        target.port: 8080
```

## Action Required

- [ ] User clarifies what "traditional configuration" refers to
- [ ] Review L4 code for existing traditional config support
- [ ] Document traditional config format (if exists)
- [ ] Decide: keep as-is vs. deprecate vs. enhance

## Impact

Configuration format compatibility, user migration path

## Complexity

TBD (depends on what needs to be done)
