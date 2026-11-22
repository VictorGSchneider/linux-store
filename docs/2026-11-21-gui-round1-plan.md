# GUI, rodada 1 — Plano de implementação

> **Para agentes:** SUB-SKILL OBRIGATÓRIA: use superpowers:subagent-driven-development (recomendado) ou superpowers:executing-plans para executar este plano tarefa por tarefa. Os passos usam caixas de seleção (`- [ ]`).

**Objetivo:** a primeira versão da GUI Slint: tela inicial (busca, chips de fonte, uma fileira por fonte), página de app que reúne todas as fontes, instalar/remover pelo pacman, Installeds e Updates.

**Arquitetura:** o `core` ganha o modelo `App` (o mesmo app em várias fontes), o `Catalog` (consultas de leitura com ordem de fontes do host) e refresh paralelo por fonte. Um crate novo `store-app` (sem Slint, testado no CI) tem o `Engine` com threads de busca, refresh, operações e ícones; a GUI só liga o Engine à interface.

**Tecnologias:** Rust 2024, rusqlite/FTS5, quick-xml, Slint 1.19, `jxl-oxide` e `png` para ícones.

**Design:** `docs/2026-11-21-gui-design.md` (aprovado em 2026-11-21). Modelos visuais: `docs/layout model *.png`.

## Restrições globais

- A GUI nunca roda como root; instalar e remover passam só pelo `Installer` do core (helper via pkexec). **O agente nunca roda `pkexec`, `sudo`, `store install|remove|upgrade`**, nem a GUI com instalação real.
- Nunca `cargo run` simples da GUI nesta máquina: use `SLINT_BACKEND=winit-software`. Compile o helper só com `--release`.
- Nada bloqueante no thread da UI. Abrir não depende de rede.
- Interface em inglês, todo texto visível em `@tr()`.
- Rodada 1 **não tem**: categorias, barra lateral, Settings, cartão duplo, `store-metadata`, capturas de tela, instalador de Flatpak/AUR.
- Chips: só fontes com pacotes no índice. Ordem de fontes: nativa do host, flatpak, depois `pacman, aur, snap, appimage`, o resto por nome.
- Juntar apps só por `app_id` do AppStream ou nome idêntico (minúsculas); um app tem no máximo uma variante por fonte. A correspondência vive na tabela `app_links` do índice, que é uma função pura de `packages` e se refaz sozinha a cada `replace_source`.
- Ícones: um único arquivo por pacote (`Package.icon`: 64×64 no pacman, 128×128 no Flatpak), usado nos cartões e na página do app (a página só o escala; um ícone maior fica para a rodada 2). Decodificados em memória (PNG e JXL), cache LRU, sem arquivos convertidos.
- Dependências novas com `cargo add` e `Cargo.lock` versionado (o CI usa `--locked`).
- Antes de cada push: `cargo fmt --all --check`; `cargo clippy --workspace --exclude linux-store --all-targets --features store-core/testing -- -D warnings`; `cargo test --workspace --exclude linux-store --features store-core/testing`; `cargo check -p linux-store`.

## Foco da revisão

Entradas que o design não nomeia e que mais provavelmente morderão. Cada uma tem o teste na tarefa dona.

1. Dois pacotes da **mesma fonte** com o mesmo `app_id` não viram um app só (o botão instalaria o pacote errado). Tarefa 7.
2. `firefox-bin` do AUR não junta com `firefox`; `Firefox` e `firefox` (caixa) juntam; as ligações se corrigem quando outra fonte chega depois. Tarefa 7.
3. Ícone ausente, truncado ou de formato desconhecido: o cartão cai para texto, sem pânico. Tarefa 11.
4. `/etc/os-release` ausente, ilegível ou com `ID` desconhecido: sem fonte nativa, e a ordem continua determinística. Tarefa 2.
5. Senha cancelada (`InstallError::Dismissed`), erro do helper ou cancelamento: a UI volta ao estado ocioso com mensagem, nunca fica em "installing". Tarefas 12 e 15.
6. Busca digitada rápido: respostas de gerações antigas são descartadas. Tarefa 12.

## Estrutura de arquivos

- `core/src/package.rs` (modificar): campos novos.
- `core/src/index.rs` (modificar): esquema v3, tabela `app_links` e consultas novas.
- `core/src/host.rs` (criar): `Host`, ordem de fontes.
- `core/src/app.rs` (criar): `App`, `assemble`.
- `core/src/catalog.rs` (criar): `Catalog`, `Row`.
- `core/src/store.rs` (modificar): refresh paralelo com progresso; ordem do host.
- `appstream/` (criar, pacote `store-appstream`): parser AppStream extraído do Flatpak.
- `backends/flatpak`, `backends/pacman`, `backends/aur` (modificar).
- `system/` (criar, pacote `store-system`): lista de backends, instalador e caminho do índice, usados pela CLI e pela GUI.
- `app/` (criar, pacote `store-app`): `Engine`, ícones, cor de acento.
- `gui/` (modificar): `ui/*.slint` em componentes e `src/main.rs` ligando ao Engine.

---

### Task 1: Campos novos em `Package` e esquema v3 do índice

**Arquivos:**
- Modificar: `core/src/package.rs`, `core/src/index.rs`, `core/src/testing.rs`, e todo literal `Package { .. }` do workspace (`backends/*/src/lib.rs`, `cli/src/lib.rs`, testes)
- Teste: `core/src/index.rs` (módulo de testes)

**Interfaces:**
- Produz: `Package` ganha `app_id: Option<String>`, `categories: Vec<String>` (categorias AppStream como escritas, ex. `"Network"`), `icon: Option<String>` (caminho absoluto de arquivo local), `popularity: Option<f64>`, `upgradable: bool`. `testing::pkg` preenche `None`, vazio e `false`.

- [ ] **Passo 1: teste que falha** `index_round_trips_new_package_fields`: grava (em memória) um `Package` com `app_id = Some("org.mozilla.firefox")`, `categories = ["Network","WebBrowser"]`, `icon = Some("/x/y.jxl")`, `popularity = Some(1.5)`, `upgradable = true` e confere que `get` devolve igual. Outro teste, `categories_with_semicolon_are_not_split_wrongly`: uma categoria contendo `;` não corrompe a lista (escolha a codificação, ex. JSON, e teste com `"A;B"`).
- [ ] **Passo 2:** rode `cargo test -p store-core --features testing index_round_trips` e veja falhar por não compilar.
- [ ] **Passo 3:** acrescente os campos; no esquema, colunas `app_id TEXT`, `categories TEXT NOT NULL DEFAULT '[]'`, `icon TEXT`, `popularity REAL`, `upgradable INTEGER NOT NULL DEFAULT 0`, mais `CREATE INDEX packages_app_id ON packages (app_id)`; `SCHEMA_VERSION = 3`; ajuste `COLUMNS`, `row_to_package` e o `INSERT`. Corrija os literais do workspace com `None`/vazio/`false`.
- [ ] **Passo 4:** rode `cargo test --workspace --exclude linux-store --features store-core/testing` e veja tudo passar, incluindo o teste existente de migração de índice antigo (um índice v2 é reconstruído).
- [ ] **Passo 5: commit** `core: Package ganha app_id, categorias, ícone, popularidade e upgradable (índice v3)`.

### Task 2: `Host` e ordem de fontes

**Arquivos:**
- Criar: `core/src/host.rs`; modificar `core/src/lib.rs`, `core/src/store.rs`
- Teste: `core/src/host.rs`

**Interfaces:**
- Produz: `pub struct Host`; `Host::detect() -> Host` (lê `/etc/os-release`); `Host::from_os_release(text: &str) -> Host`; `Host::with_native(source: &str) -> Host`; `Host::none() -> Host`; `Host::rank(&self, source: &SourceId) -> (u8, String)`; `Host::sort(&self, sources: &mut Vec<SourceId>)`; `Host::native(&self) -> Option<&SourceId>`.
- `Store::with_host(self, host: Host) -> Store`; `Store::new` usa `Host::none()`. `search_by_source` passa a ordenar com `Host::sort` (a lista fixa sai de lá).

- [ ] **Passo 1: testes que falham**: `arch_and_derivatives_map_to_pacman` (`ID=cachyos`, `ID_LIKE=arch`; `ID=arch`), `debian_family_maps_to_apt` (`ID=ubuntu`, `ID_LIKE="debian"`), `fedora_family_maps_to_dnf`, `unknown_or_empty_os_release_has_no_native` (texto vazio, lixo, `ID=nixos`), `native_first_then_flatpak_then_fixed_list` (nativa `pacman`; entrada `[aur, flatpak, snap, pacman, zzz, appimage]` sai `[pacman, flatpak, aur, snap, appimage, zzz]`), `without_native_flatpak_comes_first` (entrada `[aur, pacman, flatpak]` sai `[flatpak, pacman, aur]`).
- [ ] **Passo 2:** rode `cargo test -p store-core --features testing host` e veja falhar.
- [ ] **Passo 3:** implemente `from_os_release` (valores com ou sem aspas; `ID_LIKE` pode ter vários ids) e `rank`: nativa 0, flatpak 1, depois `pacman, aur, snap, appimage` em 2 a 5, o resto 6 com desempate por nome. `detect` lê o arquivo e, em qualquer erro, devolve `Host::none()`. Ajuste os testes existentes de `search_by_source` para `Store::new(..).with_host(Host::with_native("pacman"))`.
- [ ] **Passo 4:** rode a suíte do workspace (comando das restrições) e veja passar.
- [ ] **Passo 5: commit** `core: Host e ordem de fontes consciente do sistema`.

### Task 3: crate `store-appstream` e o Flatpak com ícone e categorias

**Arquivos:**
- Criar: `appstream/Cargo.toml`, `appstream/src/lib.rs`; modificar `Cargo.toml` (membros), `backends/flatpak/{Cargo.toml,src/lib.rs}`
- Teste: `appstream/src/lib.rs`, `backends/flatpak/src/lib.rs`

**Interfaces:**
- Produz: `store_appstream::Component { kind, id, name: Option<String>, summary: Option<String>, homepage: Option<String>, bundle: Option<String>, pkgnames: Vec<String>, categories: Vec<String>, icons: Vec<Icon>, releases: Vec<(u64, String)> }`; `Icon { kind: String /* "cached"|"stock"|"remote"|"local" */, width: Option<u32>, value: String }`; `pub fn parse_file(path: &Path, on_component: &mut dyn FnMut(Component)) -> Result<Option<String>, String>` (devolve o atributo `origin` de `<components>`, quando existe). `.gz` é lido por extensão.

- [ ] **Passo 1:** mova para o crate novo o parser de `backends/flatpak/src/lib.rs` (`parse_catalog`, `Component`, leitura de `.gz`), acrescentando `<pkgname>` (vários), `<categories>/<category>` e `<icon>` (com `width`). **Mantenha as mensagens de erro como estão** (os testes do Flatpak as conferem). Mova junto os testes de parser e ajuste-os ao novo local; rode `cargo test -p store-appstream` até passar.
- [ ] **Passo 2: testes que falham** em `appstream`: `parses_pkgnames_categories_and_icons` (XML com dois `<pkgname>`, duas categorias, ícones `cached` 64 e 128 e um `stock`), `ignores_localized_names`, `reports_origin_attribute`.
- [ ] **Passo 3:** implemente o que falta no parser até os testes passarem.
- [ ] **Passo 4: testes que falham** em `backends/flatpak`: `package_has_app_id_categories_and_icon` (catálogo de exemplo no `tempfile`, com `icons/128x128/<arquivo>.png` criado; `app_id == name`, `categories` preenchido, `icon` = `<active>/icons/128x128/<arquivo>.png`), `missing_icon_file_gives_no_icon`.
- [ ] **Passo 5:** faça o backend Flatpak usar `store_appstream::parse_file`; o ícone é o `cached` de **128** se o arquivo existir em `<active>/icons/<WxH>/`, senão `None`. Rode a suíte do workspace.
- [ ] **Passo 6: commit** `appstream: parser compartilhado; flatpak: app_id, categorias e ícone`.

### Task 4: pacman lê o catálogo AppStream e marca atualizações

**Arquivos:**
- Modificar: `backends/pacman/{Cargo.toml,src/lib.rs}`
- Teste: `backends/pacman/src/lib.rs`

**Interfaces:**
- Consome: `store_appstream::parse_file`, `Package` da Tarefa 1.
- Produz: `PacmanBackend::with_catalog_dirs(self, dirs: &[PathBuf]) -> PacmanBackend` (cada diretório tem `xml/*.xml[.gz]` e `icons/<origem>/<WxH>/`). `from_system()` usa por padrão `/usr/share/swcatalog` e `/usr/share/app-info`; `with_paths(..)` (testes) **não usa nenhum catálogo** até `with_catalog_dirs`, para os testes não dependerem da máquina.

- [ ] **Passo 1: testes que falham**: `catalog_enriches_packages_by_pkgname` (catálogo de exemplo com `<pkgname>firefox</pkgname>`, `app_id`, categorias e ícone cached 64; o pacote `firefox` do repositório de teste ganha `app_id`, `categories`, `title = Some("Firefox")` e `icon` apontando para `<dir>/icons/<origem>/64x64/<arquivo>`), `missing_catalog_changes_nothing`, `corrupt_catalog_is_ignored_not_an_error`, `package_is_upgradable_when_repo_version_is_newer` e `local_newer_than_repo_is_not_upgradable` (use o `alpm::vercmp`; os testes existentes do crate montam bancos de exemplo, reaproveite).
- [ ] **Passo 2:** rode `cargo test -p store-backend-pacman` (debug está ok: o `alpm` tem asserts desligados no `Cargo.toml` raiz) e veja falhar.
- [ ] **Passo 3:** implemente. O mapa `pkgname -> primeiro componente` vem de todos os `*.xml`/`*.xml.gz`; só tipos de aplicativo; falha de leitura do catálogo é ignorada (enriquecimento é opcional). `upgradable = vercmp(sync_version, local_version) == Ordering::Greater`. O ícone escolhido é o `cached` de 64 (se não houver, o de menor largura); se o arquivo não existe, `None`.
- [ ] **Passo 4: medir** (máquina de desenvolvimento, que tem o catálogo): `cargo build --release -p store-cli && time ./target/release/store refresh`. Registre o tempo antes e depois no corpo do commit. Se o refresh do pacman passar de ~1 s a mais, pare e avise o usuário antes de seguir.
- [ ] **Passo 5: commit** `pacman: enriquece com o catálogo AppStream e marca atualizações`.

### Task 5: popularidade do AUR

**Arquivos:** modificar `backends/aur/src/lib.rs`; teste no mesmo arquivo.

- [ ] **Passo 1: teste que falha** `popularity_is_read_from_the_dump`: um dump de exemplo com `"Popularity": 2.5` e outra entrada sem o campo; o primeiro pacote vem com `popularity == Some(2.5)`, o segundo com `None`.
- [ ] **Passo 2:** rode `cargo test -p store-backend-aur popularity` e veja falhar.
- [ ] **Passo 3:** acrescente `popularity: Option<f64>` à `Entry` (`#[serde(default)]`) e passe-o ao `Package`.
- [ ] **Passo 4:** rode a suíte do workspace.
- [ ] **Passo 5: commit** `aur: lê a popularidade do dump`.

### Task 6: `App` (montagem a partir de pacotes ligados)

**Arquivos:** criar `core/src/app.rs`; modificar `core/src/lib.rs`; teste no mesmo arquivo.

**Interfaces:**
- Produz: `pub struct App { pub key: String, pub title: String, pub variants: Vec<Package> }`; `App::icon(&self) -> Option<&str>`; `App::description(&self) -> Option<&str>`; `App::is_installed(&self) -> bool`; `App::is_upgradable(&self) -> bool`; `App::variant(&self, source: &SourceId) -> Option<&Package>`; `pub fn assemble(linked: Vec<(String, Package)>, host: &Host) -> Vec<App>`.
- `assemble` recebe pares `(app_key, pacote)` (a chave vem da tabela `app_links`, Tarefa 7) e junta os que têm a mesma chave. Os apps saem na ordem do primeiro pacote de cada chave (preserva o ranking da busca); as variantes saem ordenadas por `Host::sort`; `title` é o primeiro `title` disponível, senão o nome da primeira variante.

- [ ] **Passo 1: testes que falham**: `same_key_joins_variants`, `different_keys_stay_separate`, `apps_follow_input_order`, `variants_follow_host_order`, `title_prefers_a_variant_with_title`, `is_installed_and_upgradable_look_at_any_variant`.
- [ ] **Passo 2:** rode `cargo test -p store-core --features testing app::` e veja falhar.
- [ ] **Passo 3:** implemente `assemble` num único passe (mapa `chave -> índice`).
- [ ] **Passo 4:** rode os testes do módulo e veja passar.
- [ ] **Passo 5: commit** `core: App e montagem por chave`.

### Task 7: tabela `app_links` e consultas no `Index`

**Arquivos:** modificar `core/src/index.rs` (esquema, `replace_source`, consultas); teste no mesmo arquivo.

**Interfaces:**
- Esquema (`SCHEMA_VERSION` sobe para **4**, o índice é só cache e se reconstrói; assim um índice v3 da Tarefa 1 ganha a tabela): `app_links (source TEXT NOT NULL, name TEXT NOT NULL, app_key TEXT NOT NULL, PRIMARY KEY (source, name))` e `CREATE INDEX app_links_key ON app_links (app_key)`. A tabela é uma **função pura de `packages`**: pode ser apagada e refeita a qualquer momento.
- Regra da chave de cada pacote `p`, calculada em SQL (uma instrução, com `row_number()` para o item 2):
  1. base = `p.app_id`, senão o menor `app_id` não nulo entre os pacotes de **nome igual em minúsculas** (de qualquer fonte), senão `lower(p.name)`;
  2. dois pacotes com a mesma base e a mesma fonte: o de menor `name` fica com a base e cada outro recebe `base || '#' || name` (um app nunca tem duas variantes da mesma fonte);
  3. nada além disso (nenhuma heurística de nome parecido: `firefox-bin` não junta com `firefox`).
- Produz em `Index`: `rebuild_links(&mut self) -> Result<()>` (chamado **dentro da transação** de `replace_source`, depois de gravar os pacotes); `keys_of(&self, packages: &[Package]) -> Result<Vec<String>>` (chave de cada pacote, na mesma ordem); `members(&self, app_key: &str) -> Result<Vec<Package>>` (todos os pacotes do app); `source_counts(&self) -> Result<Vec<(SourceId, usize)>>`; `featured(&self, source: &SourceId, limit: usize, seed: u64) -> Result<Vec<Package>>`; `upgradable(&self) -> Result<Vec<Package>>`.
- `featured`: no AUR, por `popularity` decrescente; nas demais, só pacotes com `icon`, em ordem pseudoaleatória **estável** para o mesmo `seed` (ordem por `hash(seed, name)`); sem nenhum ícone, devolve os primeiros por nome.

- [ ] **Passo 1: testes que falham** (regras de ligação): `same_app_id_links_pacman_and_flatpak`, `identical_name_links_aur_to_pacman`, `name_match_ignores_case`, `package_without_app_id_joins_the_app_id_of_a_same_named_package`, `aur_firefox_bin_does_not_link_to_firefox`, `two_packages_of_one_source_with_same_app_id_stay_separate` (pacman `foo` e `foo-extra`, ambos `app_id = org.x.Foo`, mais um flatpak `org.x.Foo`: o flatpak e `foo` compartilham a chave; `foo-extra` fica com `org.x.Foo#foo-extra`), `links_are_rebuilt_when_another_source_arrives` (AUR gravado primeiro com chave `firefox`; ao gravar o pacman com `app_id`, a chave do AUR passa a ser `org.mozilla.firefox`), `replacing_a_source_removes_its_old_links`, `rebuild_is_idempotent`.
- [ ] **Passo 2:** testes que falham (consultas): `keys_of_keeps_the_input_order`, `members_returns_every_source`, `source_counts_counts_each_source`, `featured_aur_orders_by_popularity`, `featured_prefers_packages_with_icons`, `featured_is_stable_for_a_seed_and_varies_between_seeds`, `featured_falls_back_when_no_icons`, `upgradable_lists_only_flagged_packages`.
- [ ] **Passo 3:** rode `cargo test -p store-core --features testing index::` e veja falhar.
- [ ] **Passo 4:** implemente. `rebuild_links` apaga e refaz a tabela inteira com um `INSERT .. SELECT` (a regra acima), usando índices em `lower(name)` e `app_id`.
- [ ] **Passo 5: medir**: um teste `#[ignore]` (`cargo test -p store-core --features testing -- --ignored rebuild_links_timing --nocapture`) cria 150 mil pacotes sintéticos em 3 fontes e imprime o tempo de `replace_source` e de `rebuild_links`. Registre os dois no corpo do commit. Se `rebuild_links` passar de 1 s, pare e avise o usuário antes de seguir (a alternativa é refazer só as chaves tocadas pela fonte).
- [ ] **Passo 6:** rode a suíte do workspace.
- [ ] **Passo 7: commit** `core: tabela app_links e consultas de destaque, contagem e atualizações`.

### Task 8: `Catalog`

**Arquivos:** criar `core/src/catalog.rs`; modificar `core/src/lib.rs`; teste no mesmo arquivo.

**Interfaces:**
- Consome: `Index` (Tarefa 7), `Host`, `App`, `assemble`.
- Produz: `pub struct Catalog`; `Catalog::new(index: Index, host: Host) -> Catalog`; `Catalog::open(path: &Path, host: Host) -> Result<Catalog>`; `pub struct Row { pub source: SourceId, pub apps: Vec<App>, pub total: usize }`; métodos: `search(&self, query: &str, source: Option<&SourceId>, limit: usize) -> Result<Vec<App>>`; `app(&self, key: &str) -> Result<Option<App>>`; `chips(&self) -> Result<Vec<(SourceId, usize)>>` (ordem do host); `home(&self, per_row: usize, seed: u64) -> Result<Vec<Row>>` (ordem do host, fontes sem pacotes ficam de fora); `installed(&self) -> Result<Vec<App>>`; `updates(&self) -> Result<Vec<App>>`.
- Regra: cada `App` devolvido é **completo**. Os pacotes encontrados viram chaves (`keys_of`), as chaves repetidas são descartadas mantendo a ordem, e cada chave é expandida com `members`, mesmo que só uma variante tenha casado com a busca. `app(key)` é `members(key)` montado, ou `None` se vazio.

- [ ] **Passo 1: testes que falham**: `search_returns_one_app_with_variants_from_every_source` (a busca casa só o pacote pacman; o app traz também o flatpak), `search_with_source_filter_still_returns_complete_apps`, `search_of_punctuation_or_empty_returns_nothing`, `app_by_key_returns_none_for_unknown_key`, `chips_follow_host_order_and_skip_empty_sources`, `home_rows_follow_host_order`, `installed_groups_an_app_installed_in_two_sources`, `updates_lists_only_upgradable_apps`.
- [ ] **Passo 2:** rode `cargo test -p store-core --features testing catalog::` e veja falhar.
- [ ] **Passo 3:** implemente. `search` pede `limit * 3` pacotes ao índice, converte em chaves, expande e corta em `limit`.
- [ ] **Passo 4:** rode a suíte do workspace.
- [ ] **Passo 5: commit** `core: Catalog com busca, página de app, fileiras, instalados e atualizações`.

### Task 9: refresh paralelo com progresso

**Arquivos:** modificar `core/src/store.rs`; teste no mesmo arquivo.

**Interfaces:**
- Produz: `pub enum RefreshStage { Started, Finished(Outcome) }`; `Store::refresh_with(&mut self, only: Option<&[SourceId]>, progress: &(dyn Fn(&SourceId, &RefreshStage) + Sync)) -> RefreshReport`. `refresh()` passa a ser `refresh_with(None, &|_, _| {})`, com o mesmo resultado e a mesma ordem de `RefreshReport.sources` de antes.

- [ ] **Passo 1: testes que falham**: `parallel_refresh_gives_the_same_report_as_before` (o teste existente já cobre; mantenha-o verde), `progress_reports_started_then_finished_per_source`, `refresh_only_touches_the_named_sources` (a outra fonte mantém os dados antigos), `one_failing_source_does_not_stop_the_others`, e `sources_really_run_in_parallel` (dois `FakeBackend` cujo `packages()` espera numa barreira compartilhada; com execução sequencial o teste travaria, então use `Barrier` com timeout de 5 s e falha clara).
- [ ] **Passo 2:** rode e veja falhar.
- [ ] **Passo 3:** implemente com `std::thread::scope`: um thread por backend disponível chama `packages()` (e `first_of_each_name`); os resultados são gravados **um por vez** no índice, na ordem dos backends. `Started` sai antes da coleta, `Finished` depois da gravação.
- [ ] **Passo 4:** rode a suíte do workspace.
- [ ] **Passo 5: commit** `core: refresh em paralelo por fonte, com progresso`.

### Task 10: crate `store-system` e a CLI usando-o

**Arquivos:**
- Criar: `system/Cargo.toml`, `system/src/lib.rs`; modificar `Cargo.toml` (membros), `cli/{Cargo.toml,src/main.rs,src/lib.rs}`
- Teste: `system/src/lib.rs`

**Interfaces:**
- Produz: `store_system::backends() -> Vec<Box<dyn Backend>>` (pacman, flatpak, aur, nessa ordem); `store_system::installer() -> PkexecInstaller`; `store_system::default_db_path() -> Option<PathBuf>` e `default_db_path_from(xdg_cache_home, home)` (movidos de `cli/src/lib.rs` com os testes). `store_cli` reexporta os dois `default_db_path*` para não quebrar chamadores.

- [ ] **Passo 1:** mova as funções de caminho e os testes delas; faça `cli/src/main.rs` usar `store_system::backends()` e `installer()`. Rode `cargo test -p store-system -p store-cli --features store-core/testing`.
- [ ] **Passo 2:** `main` da CLI também usa `Store::with_host(Host::detect())`; um teste do CLI (`search_columns_follow_host_order`) confirma a ordem com `Host::with_native("pacman")` num `Store` de teste.
- [ ] **Passo 3:** rode a suíte do workspace e `cargo run --locked --release -p store-cli -- --help`.
- [ ] **Passo 4: commit** `system: backends, instalador e caminho do índice compartilhados; CLI usa o crate`.

### Task 11: ícones e cor de acento (`store-app`)

**Arquivos:**
- Criar: `app/Cargo.toml` (pacote `store-app`), `app/src/lib.rs`, `app/src/icons.rs`, `app/tests/fixtures/icon.png`, `app/tests/fixtures/icon.jxl`; modificar `Cargo.toml` (membros)
- Teste: `app/src/icons.rs`

**Interfaces:**
- Produz: `pub struct Rgba { pub width: u32, pub height: u32, pub pixels: Vec<u8> }` (RGBA8, `pixels.len() == width*height*4`); `icons::load(path: &Path) -> Option<Rgba>` (por extensão: `png` e `jxl`; qualquer erro é `None`); `pub struct IconCache`, `IconCache::new(capacity: usize)`, `get(&mut self, path: &str) -> Option<Arc<Rgba>>` e `put(&mut self, path: String, image: Arc<Rgba>)` (LRU); `pub fn accent(name: &str) -> (u8, u8, u8)` (determinística, cada canal entre 60 e 200).
- Dependências: `cargo add jxl-oxide png` em `store-app`.

- [ ] **Passo 1:** gere as fixtures: um PNG RGBA 8×8 (pixel `(255,0,0,128)` em todos) e, a partir dele, `cjxl icon.png icon.jxl` (do pacote `libjxl`); versione os dois.
- [ ] **Passo 2: testes que falham**: `loads_png_with_alpha`, `loads_jxl_with_alpha` (mesmo tamanho e alfa ≈128 com tolerância pequena), `missing_file_is_none`, `truncated_png_is_none`, `truncated_jxl_is_none`, `unknown_extension_is_none`, `empty_file_is_none`, `lru_evicts_the_least_recently_used`, `accent_is_stable_and_in_range`.
- [ ] **Passo 3:** rode `cargo test -p store-app` e veja falhar.
- [ ] **Passo 4:** implemente. O JXL é decodificado para RGBA8 com `jxl-oxide`; limite de 4096 px por lado (maior que isso, `None`).
- [ ] **Passo 5:** rode os testes e veja passar.
- [ ] **Passo 6: commit** `app: decodificação de ícones PNG e JXL, cache LRU e cor de acento`.

### Task 12: `Engine`

**Arquivos:** criar `app/src/engine.rs`; modificar `app/src/lib.rs`, `app/Cargo.toml` (dependências de `store-core`, `store-protocol`; dev: `store-core` com `testing`); teste em `app/tests/engine.rs`.

**Interfaces:**
- Consome: `Catalog`, `Store`, `Host`, `Installer`, `CancelToken`, `icons::load`.
- Produz:
  - `pub struct EngineConfig { pub db_path: PathBuf, pub backends: Vec<Box<dyn Backend>>, pub installer: Arc<dyn Installer>, pub installable: Vec<SourceId>, pub host: Host }`
  - `pub enum Command { Chips, Home { per_row: usize, seed: u64 }, Search { generation: u64, query: String, source: Option<SourceId> }, Open { key: String }, Installed, Updates, Refresh, Install { key: String, source: SourceId }, Remove { key: String, source: SourceId }, CancelOperation, LoadIcon { path: String, side: u32 } }`
  - `pub struct AppView { pub app: App, pub installable: Vec<SourceId> }`
  - `pub enum RefreshState { Started, Updated(usize), Unavailable(Option<String>), Failed(String) }`
  - `pub enum OpState { Started(String), Progress { message: String, percent: Option<u8> }, Finished(Result<String, String>) }`
  - `pub enum Event { Chips(Vec<(SourceId, usize)>), Home(Vec<Row>), SearchResults { generation: u64, apps: Vec<App> }, AppPage(Option<AppView>), Installed(Vec<App>), Updates(Vec<App>), Refresh { source: SourceId, state: RefreshState }, RefreshDone, Operation(OpState), Icon { path: String, image: Option<Arc<Rgba>> }, Error(String) }`
  - `Engine::start(config: EngineConfig, on_event: impl Fn(Event) + Send + Sync + 'static) -> Result<Engine, store_core::Error>`; `Engine::send(&self, command: Command)`; ao descartar o `Engine`, os threads terminam.
  - `pub fn coalesce(commands: Vec<Command>) -> Vec<Command>`: de vários `Search` seguidos mantém só o último; os outros comandos mantêm a ordem.
- Threads: **consultas** (dono do `Catalog` em conexão própria; recebe `Chips`, `Home`, `Search`, `Open`, `Installed`, `Updates`, aplica `coalesce` ao que está na fila); **refresh** (dono do `Store`; `Refresh` usa `refresh_with`, emitindo `Refresh{..}` por fonte e depois `Chips` e `RefreshDone`); **operações** (uma por vez; guarda o `CancelToken` atual num `Arc<Mutex<Option<CancelToken>>>` para `CancelOperation` funcionar durante a execução); **ícones** (2 threads, `LoadIcon` → `icons::load` → `Event::Icon`).
- `Install`/`Remove`: resolve a variante da `source` em `Catalog::app(key)`; se a fonte não está em `installable` ou a variante não existe, emite `Operation(Finished(Err(..)))` sem chamar o instalador. Senão monta `Request::Install { packages: vec![variant.name] }` (ou `Remove`) e chama `Installer::run`, traduzindo os eventos do protocolo em `OpState::Progress` (mensagem com etapa e pacote, percentual) e o resultado em `Finished`: sucesso `Ok(resumo)`; `InstallError::Dismissed` → `Err("authentication cancelled")`; `Rejected` → `Err(message)`; `ErrorCode::Cancelled` → `Err("cancelled")`; demais erros → `Err(texto do erro)`. Após o término (com ou sem sucesso) refaz o refresh só da fonte afetada e emite `RefreshDone`.

- [ ] **Passo 1: testes que falham** (com `FakeBackend`, `FakeInstaller` e um índice em arquivo temporário): `coalesce_keeps_only_the_last_search`, `coalesce_keeps_other_commands_in_order`, `chips_event_lists_sources_after_refresh`, `refresh_emits_per_source_state_then_done`, `failing_source_does_not_remove_other_chips`, `search_result_carries_its_generation`, `open_unknown_key_gives_none`, `open_lists_installable_sources`, `install_unsupported_source_never_calls_the_installer`, `install_sends_the_variant_name_to_the_installer`, `dismissed_authentication_finishes_idle_with_message`, `rejected_request_finishes_with_the_helper_message`, `cancel_during_an_operation_sets_the_token`, `icon_event_is_none_for_a_missing_file`.
- [ ] **Passo 2:** rode `cargo test -p store-app --features store-core/testing` e veja falhar.
- [ ] **Passo 3:** implemente o Engine. Os testes esperam eventos recebendo-os num canal com timeout de 5 s.
- [ ] **Passo 4:** rode `cargo test -p store-app` 20 vezes seguidas (`for i in $(seq 20); do cargo test -p store-app -q || break; done`) para pegar corrida, e a suíte do workspace.
- [ ] **Passo 5: commit** `app: Engine com threads de consulta, refresh, operações e ícones`.

### Task 13: componentes Slint e tela inicial estática

**Arquivos:** modificar `gui/ui/app-window.slint`; criar `gui/ui/{theme,card,chip,row,top-bar,home-page}.slint`; modificar `gui/Cargo.toml` e `gui/src/main.rs` (só o necessário para compilar).

**Interfaces (Slint):**
- `export struct CardData { key: string, title: string, subtitle: string, source: string, icon: image, has-icon: bool, accent: color, installed: bool }`
- `export struct ChipData { source: string, count: int, selected: bool }`
- `export struct RowData { title: string, cards: [CardData] }`
- `AppWindow` expõe `in property <[ChipData]> chips`, `in property <[RowData]> rows`, `in property <[CardData]> results`, `in property <bool> searching`, `in-out property <string> query`, e os callbacks `search-edited(string)`, `chip-clicked(string)`, `card-clicked(string)`, `show-installed()`, `show-updates()`, `show-home()`.

- [ ] **Passo 1:** instale o plugin Slint se faltar (`AGENTS.md`) e leia a skill `slint:slint` antes de escrever `.slint`. `slint-viewer` e `slint-lsp` não estão instalados nesta máquina: use o servidor MCP do plugin (ou `cargo install slint-viewer slint-lsp`) para checar e formatar.
- [ ] **Passo 2:** escreva os componentes seguindo os modelos (`docs/layout model 3.png`): fundo cinza escuro; topo fixo com os botões **Installeds** e **Updates**, a busca arredondada e centralizada, os chips com nome e contagem; abaixo, a lista rolável de `Row` (título com linha, cartões em linha, número de colunas conforme a largura). Cartão: ícone em cima e nome embaixo, etiqueta da fonte; **sem ícone**, painel com a cor `accent`, a inicial grande e o `subtitle` como texto. Settings e ☰ **não** entram nesta rodada. Todo texto fixo em `@tr()`.
- [ ] **Passo 3:** dados de exemplo só para ver: preencha as propriedades em `main.rs` com 3 fileiras de cartões (com e sem ícone) por trás de `#[cfg(debug_assertions)]` e remova na Tarefa 14.
- [ ] **Passo 4:** verifique: `cargo check -p linux-store` passa; renderize com `SLINT_BACKEND=winit-software cargo run -p linux-store` (ou o preview do plugin) e **olhe** a imagem; confira janelas estreita e larga (a grade reflui, o topo não rola).
- [ ] **Passo 5: commit** `gui: componentes e tela inicial (dados de exemplo)`.

### Task 14: ligar a tela inicial ao Engine

**Arquivos:** modificar `gui/Cargo.toml` (dependências `store-app`, `store-core`, `store-system`), `gui/src/main.rs`; criar `gui/src/convert.rs` (de `App`/`Row`/`Rgba` para as structs do Slint) e teste dele.

- [ ] **Passo 1: testes que falham** em `convert.rs` (`cargo test` do `gui` não roda no CI; rode local com `cargo test -p linux-store`): `card_for_app_with_title_and_source_label`, `card_without_icon_uses_accent_and_description`, `card_marks_installed`, `chip_labels_keep_engine_order`.
- [ ] **Passo 2:** implemente `convert.rs`.
- [ ] **Passo 3:** em `main.rs`: monte `EngineConfig` com `store_system::backends()`, `installer()`, `db_path`, `Host::detect()` e `installable = [pacman]`; callback de eventos que usa `slint::invoke_from_event_loop` com `Weak<AppWindow>`; ao abrir, envie `Chips`, `Home { per_row: 14, seed: <dia do ano> }` e `Refresh`; `search-edited` incrementa a geração e envia `Search`; resultados de geração antiga são ignorados; `chip-clicked` alterna o filtro de fonte; para cada cartão com ícone, envie `LoadIcon` e, no `Event::Icon`, crie `slint::Image` a partir de `SharedPixelBuffer<Rgba8Pixel>` **no thread da UI**. Remova os dados de exemplo.
- [ ] **Passo 4:** verifique com `SLINT_BACKEND=winit-software cargo run -p linux-store` (**sem instalar nada**): a janela abre sem esperar a rede, os chips ganham a contagem à medida que cada fonte termina, a busca responde enquanto digita, ícones aparecem. Olhe a imagem.
- [ ] **Passo 5: commit** `gui: tela inicial, busca e chips ligados ao Engine`.

### Task 15: página do app, instalar, remover e cancelar

**Arquivos:** criar `gui/ui/{app-page,operation-bar}.slint`; modificar `gui/ui/app-window.slint`, `gui/src/{main.rs,convert.rs}`.

**Interfaces (Slint):**
- `export struct SourceButtonData { source: string, version: string, installed: bool, enabled: bool, reason: string }`
- `export struct AppPageData { title: string, description: string, icon: image, has-icon: bool, accent: color, size: string, homepage: string, sources: [SourceButtonData] }`
- Propriedades: `in property <AppPageData> app-page`, `in property <bool> op-active`, `in property <string> op-message`, `in property <int> op-percent` (-1 sem percentual); callbacks `back()`, `install-clicked(string)`, `remove-clicked(string)`, `cancel-clicked()`, `open-link(string)`.

- [ ] **Passo 1: testes que falham** em `convert.rs`: `source_buttons_follow_variant_order`, `non_installable_source_is_disabled_with_a_reason` (`"no installer for this source yet"`), `installed_variant_offers_remove`, `size_is_human_readable` (0 bytes → `"—"`, 1536 → `"1.5 KiB"`).
- [ ] **Passo 2:** implemente a conversão; `card-clicked` envia `Open { key }` e o `AppPage` abre a página; um clique em instalar/remover envia o comando e mostra a barra de operação com a mensagem e o percentual; **Cancelar** envia `CancelOperation`. Ao `Operation(Finished(..))`, a barra some (sucesso ou erro) e uma mensagem de erro, se houver, aparece na página. Botões de instalar ficam desativados enquanto `op-active`. `open-link` abre a página do projeto com `xdg-open`, só para URLs `http://` ou `https://` (qualquer outro esquema é ignorado), sem esperar o processo terminar e sem passar pelo shell.
- [ ] **Passo 3:** verifique **sem instalar nada**: abra a página de um app que existe no pacman e no Flatpak (ex. Firefox, se estiver nas duas fontes) e confirme um único app com dois botões na ordem do host (pacman primeiro, flatpak desativado com o motivo); olhe a imagem. Simule a barra de operação com um `Engine` de teste ou com propriedades fixas em depuração e remova depois.
- [ ] **Passo 4: commit** `gui: página do app com um botão por fonte, instalar, remover e cancelar`.

### Task 16: Installeds e Updates

**Arquivos:** criar `gui/ui/list-page.slint`; modificar `gui/ui/app-window.slint`, `gui/src/main.rs`.

- [ ] **Passo 1:** uma página de lista rolável (cartões em grade, `results`-like) usada por **Installeds** (`Command::Installed`) e **Updates** (`Command::Updates`), com título e contagem; Updates mostra a nota "Updates are listed for pacman only" e, com zero itens, "Everything is up to date". O ☰ e o Settings não existem ainda.
- [ ] **Passo 2:** `show-installed`/`show-updates`/`show-home` mudam a página; após `RefreshDone` a página atual é pedida de novo ao Engine.
- [ ] **Passo 3:** verifique: as três páginas renderizam (olhe as imagens); `cargo check -p linux-store`.
- [ ] **Passo 4: commit** `gui: páginas Installeds e Updates`.

### Task 17: documentação, capturas e verificação manual

**Arquivos:** modificar `README.md`, `AGENTS.md`; criar `docs/screenshots/*.png`, `packaging/manual-gui-check.md`.

- [ ] **Passo 1:** capture a tela inicial, uma página de app e a lista de Installeds com renderização por software (`SLINT_BACKEND=winit-software`) e salve em `docs/screenshots/`; acrescente-as ao `README.md`.
- [ ] **Passo 2:** atualize o `AGENTS.md`: lista de pacotes (`appstream/`, `system/`, `app/`), o `Engine`, a regra de nunca instalar pela GUI pelo agente e o aviso sobre ícones JXL.
- [ ] **Passo 3:** `packaging/manual-gui-check.md`: roteiro **para o usuário** (o agente não o executa): abrir a GUI com o helper instalado (`sudo sh packaging/dev-install.sh`), instalar e remover um pacote pequeno (ex. `cowsay`) pela página do app, cancelar uma instalação, errar a senha, conferir Updates; no fim `sudo sh packaging/dev-uninstall.sh`.
- [ ] **Passo 4:** rode o comando completo das restrições globais (fmt, clippy, test, check da GUI) e cole a saída resumida no corpo do PR.
- [ ] **Passo 5: commit** `docs: capturas, AGENTS.md e roteiro de verificação manual da GUI`.

---

## Revisão do plano contra o design

- **Cobertura:** tela inicial com chips e fileiras (13–14), busca (12, 14), página do app com um botão por fonte e ordem do host (2, 6, 8, 15), correspondência por AppStream e nome (3, 4, 6, 7), instalar/remover pelo pacman e cancelar (12, 15), Installeds e Updates só do pacman (4, 8, 16), threads por fonte (9, 12), ícones em memória (11, 14), refresh que não bloqueia a UI (9, 12, 14), reorganização do core para compartilhar com a UI (10). Ficam de fora, conforme o design: categorias, barra lateral, Settings, cartão duplo, trending do Flathub, capturas de tela (rodada 2).
- **Decisão do plano:** a fileira do pacman e a do Flatpak usam pacotes com ícone em ordem pseudoaleatória estável por dia, e a do AUR usa a popularidade; o trending do Flathub entra na rodada 2.
- **Consistência de tipos:** `Host`, `App`, `assemble`, `Catalog`, `Row`, `RefreshStage`, `EngineConfig`, `Command`, `Event`, `AppView`, `OpState` e `Rgba` têm uma definição só e os mesmos nomes onde são usados.
- **Sugestão de PRs:** (A) tarefas 1–10, core, backends e `store-system`; (B) 11–12, `store-app`; (C) 13–17, GUI.
