# Diário do projeto

Cronológico, mais recente embaixo. Uma seção por dia, uma linha por fato. Registre ao fechar cada
pacote de trabalho (veja `AGENTS.md`, "Loops e documentação contínua"). Erros vão em
[`errors.md`](errors.md), decisões em [`decisions.md`](decisions.md). Entradas até 2026-11-22 foram
escritas retroativamente a partir do `git log` e dos chats; o commit entre parênteses é a prova.

## 2026-11-18 — Pacote P0 (fundação) e P1 (core, índice, pacman)
- Ideia: loja universal para Linux (de deb a AUR e mais), usando o `stoa-store` em bash como base. Brainstorming define Rust + Slint, Arch/CachyOS primeiro, Debian/Fedora previstos (D-001, D-002).
- Design aprovado e movido para `docs/` (a4bd2d9, 7464fa9): repo vira workspace Cargo com GUI Slint.
- `cargo run` simples congelou o notebook; reinício forçado. Contornado com Mesa EGL ou `winit-software` (E-001, d52d9cc).
- README em inglês e link da documentação do Slint (5f40a8d).
- Plano de `core`, índice e pacman, 6 tarefas (820c58d). Execução: modelo e trait `Backend` (091002a), índice SQLite (d5bd842), busca ranqueada (51aa425), `Store` (f563f5e), backend pacman via libalpm (cdb1ff7), CLI `store` (a931222).
- Revisão: sync db corrompido apagava o repo do índice; abrir o índice pedia lock de escrita (E-002, 7e81169).
- CI: rustfmt, clippy, testes no Arch, checagem da GUI, audit e dependabot (b7d8bdd). Dependabot sobe `actions/checkout` 4 → 7 (#2).

## 2026-11-20 — Pacotes P2 (AUR e Flatpak) e P3 (helper privilegiado)
- Proteção da `main` ativada com CI verde; a partir daqui tudo entra por branch, PR e CI (D-007).
- Plano AUR + Flatpak, 4 tarefas (7cf20b7). `Package.title` e índice v2 (78b2675), AUR pelo dump oficial (5fca3ae), Flatpak pelo catálogo AppStream (1672656), CLI busca nas três fontes (05ab0d5).
- Revisão achou apps Flatpak legados não indexados, catálogo vazio tratado como sucesso, ranking por título (E-003, a9e1cce).
- Design do helper privilegiado (4c7929a). Medição: `pkexec` com ação própria `auth_admin_keep` guarda a senha por processo; PackageKit testado e descartado para pacman (D-003, D-004, D-005).
- Sondas `pkexec` feitas pelo agente abriram prompts de senha sem aviso (E-004; regra permanente na memória).
- Plano do helper, 6 tarefas (0c0581a). Execução: protocolo (3f48043), instalação e cancelamento (8ad1549), remover/atualizar e conjunto protegido (07ef5e4), laço de requisições, aborto e auditoria (e324c08), `Installer` e cliente pkexec (b2c6b62), CLI install/remove/upgrade (f210358).
- Revisão de segurança em três rodadas (3ed0cb2, 8bc1bf9, 7e3fad0, 1b06b18): umask vazando para a libalpm, eventos bloqueando a transação, controle de terminal vindo de repositórios, `dev-install.sh` confiando em arquivos graváveis (E-005 a E-008).

## 2026-11-21 — Fechamento do P3, P4 (minors) e início do P5 (GUI)
- Teste manual real com senha passou duas vezes; `manual-check.sh` ganha mensagens sobre sobras de uma execução cancelada (E-009, 041fd8d).
- `git push` direto na `main` recusado pela proteção (GH006); fluxo vira branch + PR (E-010).
- P4: minors das etapas 1 a 3: argv não UTF-8, fonte indisponível explica o motivo, migração do índice em uma transação, CRC do gzip, timeouts e TLS do sistema no AUR, AppStream mais completo (16c5ca9, 408a4be). Busca em colunas por fonte e correção do ranking de consultas curtas (1a1d5d8, E-011). PRs #4 (helper) e #5 (P4) fundidos; 198 testes.
- Chat anterior ficou grande; recap gravado e nova sessão aberta.
- Usuário desenha três modelos de interface (`docs/layout model *.png`). Brainstorming da GUI: barra lateral sobreposta, fileira por fonte, página única do app com um botão por fonte, ordem de fontes pelo SO (D-008, D-009).
- Ícones do catálogo do Arch são JPEG XL: decodificar em memória com `jxl-oxide` (D-010). Tabela `app_links` para ligar o mesmo app entre fontes (D-011).
- Plano da rodada 1 da GUI, 17 tarefas (7b4d101, 439f7d5). Execução por subagentes na worktree `gui-design`: campos novos e índice v3 (efcab65) até `Engine` (472f990, 98a98e8).

## 2026-11-22 — P5 (GUI, rodada 1)
- Componentes e tela inicial (9592f5b), busca e chips ligados ao Engine (bd839e1), página do app com um botão por fonte (8d472e3), páginas Installeds e Updates (c3fe5de), listas grandes com teclado (62c139c).
- Estado de página e operação puro: erros não mexem na operação e respostas de páginas antigas são descartadas (416767a).
- Core: `Catalog` lê um snapshot e o refresh aceita fontes a recoletar (ae249eb); botões esperam o reload pós-operação (88e6a3c).
- CI da GUI compila todos os alvos e roda os testes da GUI (08895b1); capturas do README refeitas (7684d63).
- Branch `worktree-gui-design`: 41 commits à frente da `main`, ainda não fundida.
- Adicionados loops de trabalho e documentação contínua (este diário, `errors.md`, `decisions.md`, `STATUS.md`, painel HTML).
