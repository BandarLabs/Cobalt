# Settings runtime validation

Unmodified source `a2d76b1463574e6eee5fd28e9a910850d41124dd`: runtime-100 and runtime-170 verify Wi-Fi nested Back remains Settings beyond the host grace period, then root Back returns Launcher. The production simulator exposes no Wi-Fi networks.

The separate runtime-enterprise-* journeys use exactly `enterprise-fixture.patch` to add a synthetic eduroam network to the simulator policy. Certificate/probe/join handling and credential dispatch are unchanged. All credentials are synthetic. This is validation-only fixture coverage, not a production simulator fixture or physical-device test.

At 170%, the initial enterprise journey found a real font TextOverflow in the trust explanation (runtime-enterprise-170/failed-line-6.png). Initial 100% joined successfully but the harness incorrectly expected “Connected to eduroam”; the actual UI says “Connected eduroam.” The corrected harness checks the connected row’s “Tap to disconnect” state, and the completed pre-repair100 run is runtime-enterprise-100-final.

Production repair `8bc539807d67b775eeab5688fb2b5bdcb5599bf7`, applied to the pinned source as `trust-repair.patch`, shortens the warning while preserving fingerprint verification and exact-server password binding. Repaired100 and repaired170 journeys both pass refusal, retry, explicit trust/join, nested Back, and root return. The 170% after screenshot was visually inspected: complete warning, full SHA256, and both buttons are visible.

Each successful result.json records source, CLI hash, scale, fixture/repair patch hashes where applicable, and hardware=false. Repaired results also record the Settings binary hash; the CLI hash stays unchanged because the production repair changes the launched app, not CLI code. No fixture was committed to a code PR.

## Final long-server repair

A real-font matrix with PR236’s long server CN reproduced additional overflow after the short-server warning repair. Final production commit `e2a05090c1cdd41ca38f25a52dea0263e6fa3403` shows a measured, explicitly ellipsized server-name preview while retaining the complete fingerprint and verification warning. The full certificate CN is not displayed when it exceeds one line; the fingerprint remains the trust anchor.

Final actual runtime evidence is runtime-enterprise-long-100-final and runtime-enterprise-long-170-final, using enterprise-fixture-final.patch (synthetic network plus long certificate CN) and trust-repair-final.patch on source a2d76b1463574e6eee5fd28e9a910850d41124dd. Both refuse/retry/trust/back journeys pass. The final170 trust screenshot was visually inspected. Earlier failed and intermediate captures remain preserved.

Real-font regression covers all nine profile identities at100% and170%, a long server name, a253-character wide server name, and the waiting screen. It asserts no layout errors, complete fingerprint preservation, visible ellipsis for the extreme name, and both decision targets. Settings49 tests and strict Clippy pass. A32-byte enterpriseSSID is unreachable: enterprise selection recognizes only eduroam and govroam; ordinary SSID list identity/pagination is covered separately.
