# Status

Gerado por `scripts/gen-dashboard.py` a partir de `docs/log/progress.json`. Não edite à mão.
Atualizado em 2026-11-22. Painel: [`docs/log/dashboard.html`](docs/log/dashboard.html).

- Tarefas planejadas concluídas: **35/35**
- Testes: **198 passam, 0 falham** (main, cargo test --workspace --exclude linux-store --features store-core/testing)
- Erros registrados: 11 ([errors.md](docs/log/errors.md)); decisões: 12 ([decisions.md](docs/log/decisions.md))
- Diário: [journal.md](docs/log/journal.md)

| Pacote | Nome | Tarefas | Estado | Datas |
|---|---|---|---|---|
| P0 | Fundação: workspace, README, CI | 1/1 | fundido | 2026-11-18 |
| P1 | Core, índice SQLite e backend pacman | 6/6 | fundido | 2026-11-18 |
| P2 | Backends AUR e Flatpak (leitura) | 4/4 | fundido | 2026-11-20 |
| P3 | Helper privilegiado (pkexec) | 6/6 | fundido (PR #4) | 2026-11-20 a 2026-11-21 |
| P4 | Minors das etapas 1 a 3 e busca por fonte | 1/1 | fundido (PR #5) | 2026-11-21 |
| P5 | GUI Slint, rodada 1 | 17/17 | pronto, não fundido (41 commits) | 2026-11-21 a 2026-11-22 |
| P6 | Backends apt e dnf, adaptador PackageKit | 0/0 | não planejado |  |
| P7 | Snap, AppImage, firmware, drivers, AUR com compilação | 0/0 | não planejado |  |

## Pendências
- Remote Flatpak quebrado falha o refresh inteiro, de propósito; exigiria guardar dados por remote
- Latência da busca com prefixo de 3 letras não medida
- Kernel e bootloader fora do conjunto protegido do helper e IgnorePkg em nomes explícitos do install: não verificados
- Cópia privada do banco para o upgrade poder ser cancelado depois do refresh
- Fundir a branch worktree-gui-design (PR, CI verde, merge sem squash)
