# - Prefers the assistant to execute the work directly using the best-educated route rather than presenting a plan first; explicitly asks to “tackle it” and “don’t plan.” Confidence: 0.99
- Prefers the assistant to execute the work directly using the best-educated route rather than presenting a plan first; explicitly asks to “tackle it” and “don’t plan.” Confidence: 0.98
- Wants changes kept local or confined to their own fork; do not push or commit upstream without explicitly asking first. Confidence: 0.98
- Prefers complete, evidence-based investigations for bugs and regressions, including deeper diagnosis when an initial fix does not resolve the issue. Confidence: 0.95
- Values verification as part of implementation: rebuild or run the relevant tests and clearly confirm whether a refresh, rebuild, or app restart is needed to observe changes. Confidence: 0.92
- Prioritizes local development simplicity and resource efficiency, preferring the regular app workflow without unnecessary Docker or heavyweight services. Confidence: 0.9
- Frequently requests security-vulnerability reviews for code and configuration changes, indicating security should be checked proactively during implementation. Confidence: 0.88
- Expects credentials and API keys to be treated securely: never echoed, redacted in diagnostics, rotated when exposed, and stored via environment references rather than plaintext configuration. Confidence: 0.99
- Prefers durable root-cause fixes that prevent recurrence over temporary workarounds that merely bypass a failing path. Confidence: 0.96
