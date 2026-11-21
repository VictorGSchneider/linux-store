# GUI (Slint) — Design

Data: 2026-11-21 · Estado: rascunho para aprovação
Complementa `docs/2026-11-18-linux-store-design.md` (etapa 3 das "Etapas") e usa o helper de
`docs/2026-11-20-privileged-helper-design.md`.

## Intenção e porquê

O `core`, os três backends (pacman, AUR, Flatpak) e o helper privilegiado existem, mas só a CLI os usa. A GUI é o que faz a loja aparecer para "qualquer pessoa". O que o usuário pediu, com as palavras dele: uma tela inicial com busca, chips de fonte e fileiras de apps; categorias como no GNOME Software; e **uma página por app que reúne todas as fontes** em que ele existe, com um botão de instalar por fonte. O layout vem dos modelos em `~/Documents/Claude/` (`layoutmodel.png`, `layout model 2.png`, `layout model 3.png`).

## Objetivos

- Tela inicial: topo fixo com ☰, botões Installeds / Updates / Settings, busca e chips de fonte (nome e contagem de pacotes). Abaixo, a faixa de categorias e **uma fileira por fonte**. A tela rola na vertical.
- Busca instantânea, enquanto o usuário digita; o chip de fonte funciona como filtro.
- Barra lateral que aparece **por cima** (overlay) com os filtros. Nesta versão, só filtros.
- Cartões com imagem ou ícone quando existir, e **cartão só de texto** bem formatado quando não existir. Nome do app embaixo da imagem. Existe também um **cartão duplo** (o dobro da largura) com nota, descrição e botão, usado onde há dado de sobra.
- **Um app, uma página**, mesmo existindo em várias fontes. A página tem um botão de instalar por fonte.
- **Ordem das fontes** (chips, fileiras, botões da página do app): primeiro a fonte nativa do sistema (detectada por `ID`/`ID_LIKE` de `/etc/os-release`; pacman no Arch/CachyOS), depois o Flatpak, depois uma lista fixa (aur, snap, appimage, demais por nome). A tabela fica no core, então um backend novo não exige mexer na GUI.
- Instalar, remover e atualizar pelo helper existente, com progresso por eventos e botão cancelar.
- Dados de destaque pela rede: trending do Flathub e popularidade do AUR. Aprovado pelo usuário.

## Não-objetivos (primeira versão)

- Capturas de tela (a página do app inteira com galeria fica para uma segunda rodada; a primeira entrega ícone, textos, tamanhos e links).
- Avaliações (ODRS) e Repology.
- Backends novos (snap, AppImage, apt, dnf). Os chips deles aparecem esmaecidos com o motivo de indisponibilidade, se o backend não existir. *(minha decisão: o design original só mostra fontes disponíveis, mas o modelo do usuário mostra todos os chips.)*
- Categorias para o AUR e para pacotes do pacman sem catálogo AppStream.
- Tema escuro/claro configurável, internacionalização, acessibilidade avançada.

## Anti-objetivos (seria falha mesmo funcionando)

- A UI travar ou abrir devagar: nada bloqueante no thread da interface.
- A GUI rodar como root, ou rodar `pkexec`/helper fora do `Installer` do core.
- Juntar dois pacotes que não são o mesmo app (o botão instalaria a coisa errada). Na dúvida, **não juntar**.
- Adicionar um backend exigir mexer na GUI.
- Depender de rede para abrir: sem rede, a loja usa o índice e o cache anteriores.

## Restrições do código atual

- `Backend` é `Send + Sync` e `packages()` é bloqueante. O pacman abre um handle do libalpm por chamada; o AUR usa `ureq` com timeouts. Um thread por fonte é seguro.
- `rusqlite::Connection` não é `Sync`. O índice usa WAL, então um leitor não espera o escritor; cada thread abre a própria conexão.
- O índice é só cache e é reconstruído quando `SCHEMA_VERSION` muda (hoje 2), então acrescentar colunas é barato.
- `Store::search_by_source` hoje ordena `pacman, flatpak, aur` com uma lista fixa; passa a usar a ordem do host.
- O dump do AUR já baixado (`packages-meta-ext-v1.json.gz`) traz `NumVotes` e `Popularity`: a fileira do AUR não exige pedido novo.
- `Installer::run` entrega `Event`s e aceita `CancelToken`. `store_protocol::Request` tem só `Install`, `Remove`, `Upgrade` e `Cancel`, sem nome de fonte: **o helper instala apenas do pacman**. Flatpak e AUR ainda não têm instalador (o design do helper deixou os dois de fora de propósito).
- `installed_version`: o pacman e o Flatpak preenchem; o AUR deixa sempre `None`, mas um pacote do AUR já instalado aparece pelo pacman como instalado sem repositório (`repo: None`).
- Regras do projeto: nunca rodar `pkexec`, `sudo` nem `store install|remove|upgrade` pelo agente; nunca `cargo run` simples da GUI nesta máquina (usar `SLINT_BACKEND=winit-software` ou o EGL do Mesa); compilar o helper com `--release`.

## Abordagens consideradas (agrupamento do mesmo app entre fontes)

- **A. Agrupar no índice do core, por vínculos do AppStream, com nome exato como complemento. Escolhida.** O AppStream do pacman traz `<pkgname>` e o id do componente, que bate com o id do Flatpak. O índice grava um `app_id` por pacote. Confiável, sem rede, e a CLI também aproveita. Limitada ao que o AppStream cobre.
- **B. Agrupar na GUI por heurística de nome.** Rápida, mas frágil (`firefox` contra `org.mozilla.firefox`) e deixa a lógica na camada errada. Descartada.
- **C. Repology para o mapeamento.** Cobre mais, mas depende de rede, é lento e erra com nomes ambíguos. Descartada por ora; o `app_id` no esquema deixa a porta aberta.

## Design

### Modelo de dados (core)

- `Package` ganha `app_id: Option<String>`, `categories: Vec<String>`, `icon: Option<String>` (caminho local ou nome do ícone) e `popularity: Option<f64>`.
- O esquema do índice sobe para a versão 3 com as colunas novas e um índice em `app_id`.
- Novo tipo `App`: um nome de exibição, o `app_id` e a lista de variantes (`Package` por fonte) **já ordenada pela ordem do host**. O core oferece `Store::apps(query, limit)`, `Store::apps_in_source(...)` e `Store::app(id)`.
- Regra de agrupamento, em ordem: (1) mesmo `app_id` vindo do AppStream (componente do pacman com `<pkgname>`; id do Flatpak); (2) nome idêntico (normalizado em minúsculas). Nada além disso. O AUR só junta por nome idêntico (`firefox-bin` fica separado).
- A ordem das fontes é uma função do core: nativa do host, flatpak, depois a lista fixa. A fonte nativa vem de `/etc/os-release`.

### AppStream

- O parser hoje dentro de `backends/flatpak` vai para um crate compartilhado `store-appstream`. O backend do pacman passa a ler `/usr/share/swcatalog` (e `/usr/share/app-info`) quando existir, com nome, ícone, categorias e `<pkgname>`. O Flatpak passa a extrair também ícone e categorias.
- Sem catálogo (`archlinux-appstream-data` ausente), o pacman fica sem ícone e sem categoria; o pacote é dependência opcional, e a máquina de desenvolvimento já o tem.
- Medir o custo no refresh. Se pesar, só reler quando o arquivo do catálogo mudar.

### Metadados de rede (crate novo `store-metadata`)

- O `core` continua sem rede. `store-metadata` busca trending do Flathub (API pública do Flathub) e usa a popularidade do AUR já presente no índice.
- Cache em `$XDG_CACHE_HOME/linux-store/` (mesma pasta do índice), com validade; sem rede usa o último cache. Imagens são baixadas sob demanda e decodificadas fora do thread da UI.
- Os pedidos usam um cliente com cabeçalho genérico, sem dados do usuário.

### Threads

1. **UI** (laço de eventos do Slint): só renderiza e atualiza os modelos.
2. **Busca**: um thread com conexão de leitura. Cada tecla manda uma geração; respostas de gerações antigas são descartadas. O resultado volta por `slint::invoke_from_event_loop`.
3. **Refresh**: um coordenador abre um thread por fonte (`thread::scope`), junta os pacotes e **grava um escritor por vez**. A cada fonte pronta, a UI recebe um evento: o chip muda de "atualizando" para a contagem, e a tela refaz a consulta.
4. **Rede de metadados**: pool pequeno e fixo (4 threads).
5. **Operações**: fila de uma operação por vez, cada uma com seu `CancelToken`; os `Event`s do helper viram progresso. Não se paraleliza (o pacman trava o banco; uma senha por vez).

### Telas

- **Início**: topo fixo (☰, Installeds, Updates, Settings, busca, chips). Faixa de categorias abaixo (games, productivity, edition, media…; nomes amigáveis mapeados das categorias do freedesktop). Fileiras por fonte, cada uma com título e linha, e um "ver tudo". O número de cartões por fileira se adapta à largura.
- **Fileiras (propostas)**: flatpak = em alta no Flathub; aur = mais populares (votos/popularidade); pacman = apps com ícone/categoria quando há catálogo, senão os atualizados mais recentemente.
- **Categoria**: grade rolável só com aquela categoria.
- **Busca**: mostra apps (já agrupados) em grade; o chip de fonte filtra.
- **Installeds** e **Updates**: listas. Updates exige algo novo no core (lista de pacotes cuja `version` difere da `installed_version`, só para as fontes cuja comparação de versão é confiável; ver "Deixado ao construtor").
- **Página do app**: nome, ícone, descrição, tamanhos, links, e **um botão por fonte**, na ordem do host. Um clique inicia a operação e mostra progresso e cancelar.
- **Barra lateral (☰)**: overlay com filtros (só instalados, só com atualização, por fonte).
- **Settings**: fontes (disponíveis ou não, com o motivo), refresh manual.

### Cartões

- **Simples**: imagem ou ícone em cima, nome embaixo, etiqueta pequena da fonte. Sem imagem, vira cartão de texto: cor derivada do nome, inicial grande, descrição curta.
- **Duplo** (2× largura): ícone, nome, nota (quando houver), descrição e botão. Usado nos destaques do Flathub; as outras fileiras usam o simples.

## Fluxo de trabalho

- Branch, PR e CI verde; a `main` é protegida.
- Antes do push: `cargo fmt --all --check`, `cargo clippy --workspace --exclude linux-store --all-targets --features store-core/testing -- -D warnings`, `cargo test --workspace --exclude linux-store --features store-core/testing`, e a checagem de compilação da GUI.
- Mudanças em `.slint`: `slint-viewer --check .`, `slint-lsp format -i`, e renderizar antes de dar por pronto. Se a UI mudar visivelmente, atualizar os screenshots do `README.md`.

## Decisões do usuário (2026-11-21)

- **Escopo: só a GUI por ora.** Instalar vem só do pacman; Flatpak e AUR ficam como visualização, com o botão desativado e o motivo. Instaladores para eles são planos seguintes.
- **Duas rodadas.** Rodada 1: `App` e agrupamento no core, tela inicial (busca, chips, fileiras), página do app, instalar/remover pelo pacman, Installeds e Updates (só pacman). Rodada 2: categorias, barra lateral de filtros, Settings, cartão duplo, `store-metadata` (trending do Flathub e cache de imagens).
- **Pode reorganizar o core e os crates** quando isso facilitar o compartilhamento entre a UI e o core (por exemplo, extrair a montagem dos backends que hoje está no `main` da CLI para um lugar comum, mover o parser AppStream, dividir `Store` em escrita e leitura). Mantendo a CLI funcionando e os testes verdes.
- O catálogo `archlinux-appstream-data` está instalado na máquina de desenvolvimento (`/usr/share/swcatalog/xml/{core,extra,multilib}.xml.gz`, ícones em `/usr/share/swcatalog/icons/`). Os testes continuam usando fixtures; a máquina serve para a verificação visual e para medir o custo do refresh.

## Decisões do autor do design (minhas, para o usuário conferir)

- **Idioma da interface**: inglês, como nos modelos (Installeds, Updates, Settings). Todo texto visível passa por `@tr()` do Slint, para traduzir depois sem reescrever.
- **Updates na primeira versão**: só fontes cuja comparação de versão é confiável, ou seja, o pacman (a comparação é a do libalpm). Flatpak e AUR ficam de fora da lista de atualizações e do filtro "com atualização", e a tela diz isso. Pacotes VCS do AUR (`-git`) não têm comparação confiável.
- **Botão de instalar por fonte**: ativo só para fontes que o `Installer` suporta (hoje o pacman). Nas demais, o botão fica desativado com o motivo ("sem instalador ainda") e, quando houver `url`, um link para a página do projeto.
- **Testes de agrupamento e de ícones** usam arquivos de exemplo (fixtures), não a máquina.
- **Verificação visual**: renderização por software (`SLINT_BACKEND=winit-software`) ou pelo EGL do Mesa; as capturas novas vão para `docs/screenshots/` e o `README.md` aponta para elas.
- **Trending do Flathub**: o construtor escolhe o endpoint público e valida a resposta com tolerância; se falhar, a fileira cai para os dados locais e a falha não aparece como erro. Cache de um dia.

## Deixado ao construtor

- Divisão em planos (o core com `App`, AppStream e agrupamento tem valor sozinho e vem antes da UI).
- Número de colunas e tamanhos exatos dos cartões, cores, tipografia, animações; a aparência segue os modelos cinzas como esqueleto, não como estilo final.
- O formato exato do cache e da validade dele.
- A lista final de categorias amigáveis e o mapeamento das categorias do freedesktop.
