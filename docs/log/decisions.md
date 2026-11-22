# Decisões técnicas

Formato: contexto, decisão, motivo, alternativas. Número `D-NNN` sequencial; uma decisão trocada
não é apagada, ganha uma linha "Substituída por D-NNN". Entradas até 2026-11-22 são retroativas.

## D-001 — Rust no núcleo, Slint na GUI
- **Contexto:** loja leve, rápida, de código aberto; usuário já decidido por Slint ("nem que eu tenha que criar o que precisar").
- **Decisão:** workspace Cargo; `core` independente de UI; a GUI só fala com o core.
- **Motivo:** binário único e rápido, sem runtime pesado, e o core reaproveitável pela CLI.

## D-002 — Arch/CachyOS primeiro, backends plugáveis
- **Decisão:** trait `Backend` e índice SQLite com FTS por fonte; pacman, AUR e Flatpak agora, apt e dnf depois, só as fontes disponíveis na distro aparecem.
- **Alternativa recusada:** acoplar ao pacman.

## D-003 — Helper privilegiado próprio, via pkexec
- **Contexto:** instalar e remover exige root; única parte do código que o terá é `store-helper`.
- **Decisão:** helper com protocolo JSON de uma linha por mensagem, toda validação em `store-protocol`, ação polkit própria `auth_admin_keep` com `exec.path`.
- **Motivo:** superfície pequena e auditável; flags de teste (`--root`, `--dbpath`) existem só sob `#[cfg(test)]`.

## D-004 — Cache de senha por processo, risco aceito
- **Medição:** segunda chamada do mesmo processo levou 0,03 s sem senha; outro processo pediu de novo.
- **Decisão:** aceitar o cache de 5 minutos limitado ao processo da GUI; a validação vale em toda chamada.

## D-005 — PackageKit fora do caminho do pacman
- **Medição:** o backend alpm dele recusa pacotes `x86_64_v3` do CachyOS, reporta atualizações falsas e leva 3 a 4 s no `GetUpdates`; a libalpm direta planejou os mesmos pacotes sem erro.
- **Decisão:** libalpm direta no pacman; PackageKit fica como adaptador futuro de Debian e Fedora atrás do trait `Installer`.
- **Alternativa considerada:** embutir o PackageKit na loja; adiada.

## D-006 — Compilar o helper sempre com `--release`
- **Motivo:** o crate `alpm` falha uma checagem de alinhamento em debug (`[profile.dev.package.alpm]`).

## D-007 — `main` protegida; PR com CI verde, merge sem squash
- **Motivo:** três checks obrigatórios; sem squash o histórico por tarefa fica rastreável. Depois do merge, `git pull --rebase`.

## D-008 — Página única por app, botões por fonte
- **Decisão:** se o app existe em várias fontes, uma página só, com um botão de instalação por fonte. A ordem (chips, fileiras, botões) é por SO: primeiro a fonte nativa e o Flatpak, depois as demais; só aparecem as fontes disponíveis.

## D-009 — Barra lateral sobreposta; fileira por fonte na tela inicial
- **Decisão:** a barra lateral aparece por cima, com filtros; a tela inicial tem uma fileira por fonte e aba de categorias como o gnome-software; dados de destaque pela rede (Flathub, AUR), uso novo de rede confirmado pelo usuário.

## D-010 — Ícones JPEG XL decodificados em memória
- **Contexto:** o catálogo AppStream do Arch traz ícones JXL; converter sob demanda, usar outro formato e pôr conversor nas deps do PKGBUILD foram considerados.
- **Decisão:** decodificar em memória com `jxl-oxide`, cache LRU, sem conversor externo.

## D-011 — Tabela `app_links` liga o mesmo app entre fontes
- **Decisão:** índice por fonte mais uma tabela de correspondências (chave de app) em vez de fundir pacotes na origem; `Catalog` monta o `App`. Índice v3.

## D-012 — Documentação contínua e loops de trabalho
- **Decisão:** diário, erros, decisões, `STATUS.md` e painel HTML atualizados a cada pacote; execução, monitoramento e correção em loops separados, o monitor sem ler saída do compilador e o de correção lendo só o trecho com o erro (ver `AGENTS.md`).
- **Motivo:** chats longos perdem contexto; o estado do projeto precisa viver no repositório.
