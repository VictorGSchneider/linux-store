# linux-store — Design

Data: 2026-11-18 · Estado: rascunho para aprovação

## Intenção e porquê

Uma loja de aplicativos universal, gráfica e de código aberto para Linux, no
estilo Microsoft Store / App Store / Play Store, mas melhor: qualquer pessoa
procura e instala software com facilidade, vindo de **qualquer** fonte (repos
da distro, AUR, Flatpak, Snap, AppImage e mais), incluindo drivers, firmwares e
ferramentas de linguagens de programação.

Hoje GNOME Software e KDE Discover cobrem pouco: não têm AUR de verdade, nem
linguagens, nem uma visão única de todas as fontes. Esse é o espaço do projeto.

Ponto de partida: o `stoa-store` (bash + yad, em `stoa-store/` e `stoa-store.sh`),
que delega busca/instalação ao bauh. Serve de referência de lógica e ideias,
não de arquitetura.

## Objetivos

- App com GUI simples, para qualquer pessoa.
- **Leve e rápido**: busca instantânea, baixo uso de memória, um binário só.
- Conectar-se aos repositórios nativos da distro e ao AUR, Flatpak, Snap,
  AppImage; depois drivers, firmwares e gerenciadores de linguagens.
- Primeiro Arch/CachyOS, mas com Debian (apt) e Fedora (dnf) já previstos na
  arquitetura.
- Código aberto (GPLv3, licença já presente no repositório).

## Não-objetivos (por enquanto)

- Suporte funcional a apt/dnf na primeira etapa (só o encaixe na interface).
- Loja própria de hospedagem de pacotes: a loja **consome** fontes existentes.
- Pagamentos/apps pagos, contas de usuário, avaliações online.
- Janela web/Electron/Tauri.

## Anti-objetivos (seria falha mesmo funcionando)

- Lenta ou pesada: se abrir devagar ou a busca engasgar, falhou.
- Rodar a GUI como root.
- Parsing frágil de saída de texto de CLIs como base do sistema (o que torna o
  bauh instável).
- Adicionar um backend exigir mexer na GUI.

## Abordagens consideradas

- **A. GUI fina sobre PackageKit + Flatpak + fwupd.** Rápida de fazer, mas
  pacman fraco e sem AUR/snap/AppImage/linguagens/drivers. Descartada.
- **B. Núcleo próprio com adaptadores por backend + índice unificado.**
  Mais trabalho inicial; sustenta "tudo e leve" e permite crescer.
  **Escolhida.**
- **C. GUI que chama comandos (pacman, yay, flatpak).** Rápida de começar,
  frágil e lenta de unificar. Descartada.

## Decisões tomadas (por você)

- Abordagem B.
- Rust em tudo.
- GUI com **Slint** (mesmo que seja preciso construir componentes próprios).
- Começar por Arch/CachyOS, deixando Debian/Fedora previstos.

## Arquitetura (proposta, minhas decisões de desenho)

Workspace Cargo com:

- `core`: o trait `Backend` (buscar, detalhes, instalar, remover, atualizar,
  listar instalados), modelo comum de pacote, fila de operações com progresso
  e cancelamento, índice local em SQLite (busca offline e instantânea).
- `backends/pacman` (via libalpm), `backends/aur` (RPC + build), `backends/flatpak`
  (libflatpak/API): primeira etapa.
- `backends/apt`, `backends/dnf`: apenas esqueleto que compila e registra o
  backend como "indisponível".
- `helper`: processo privilegiado via polkit/D-Bus que executa as operações
  que exigem root. A GUI nunca roda como root.
- `gui`: Slint, conversa só com o `core`.

Detecção de backends em tempo de execução: a loja mostra só as fontes
disponíveis na distro atual.

## Etapas (cada uma vira seu próprio plano)

1. `core` + índice SQLite + backend pacman, com CLI de teste.
2. Backends AUR e Flatpak.
3. GUI Slint: busca, página do app, instalar/remover, atualizações.
4. Helper polkit (pode ser antecipado: a etapa 1 já precisa de privilégio).
5. Esqueletos apt/dnf; depois implementação real.
6. snap, AppImage, fwupd (firmware), drivers, gerenciadores de linguagens.

Este documento vale para o projeto todo; o primeiro plano de implementação
cobre só as etapas 1 e 4.

## Em aberto, para quem constrói

- Como o AUR instala: delegar a um helper existente (yay/paru) ou compilar
  com makepkg por conta própria. Sugestão: makepkg direto, pelo helper,
  para evitar dependência externa.
- Fonte dos metadados/ícones/screenshots (AppStream, Flathub, repos).
- Componentes Slint que não existem (listas virtualizadas grandes, grid de
  cartões): a construir conforme necessário.
- O que fazer com `stoa-store*`, `stoa-bauh.py`: sugestão de movê-los para
  `legacy/` e consultá-los como referência, sem apagar.
