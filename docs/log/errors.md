# Registro de erros

Formato fixo: sintoma, causa, correção, prova. Um erro novo recebe o próximo número `E-NNN`. O
loop de correção (`AGENTS.md`) escreve aqui a cada ciclo, mesmo quando a causa era banal. Erros
anteriores a 2026-11-22 foram reconstruídos dos commits e dos chats.

## E-001 — `cargo run` congela o notebook
- **Quando:** 2026-11-18, P0.
- **Sintoma:** o primeiro `cargo run` da GUI reiniciou o PC; mouse travou por segundos e nada ficou no journal.
- **Causa:** suspeita forte, não reproduzida de propósito: renderizador femtovg (OpenGL) abrindo o contexto EGL pelo driver NVIDIA (RTX 4050 + AMD 680M, Wayland/niri).
- **Correção:** usar `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json` ou `SLINT_BACKEND=winit-software`; ambos rodaram normal. Documentado no `AGENTS.md` (d52d9cc).

## E-002 — Sync db corrompido apagava o repo do índice
- **Quando:** 2026-11-18, P1.
- **Sintoma:** com o banco de sincronização ausente ou corrompido, o backend devolvia catálogo vazio e o refresh apagava o repo do índice.
- **Causa:** erro tratado como "catálogo vazio". Além disso, abrir um índice existente pedia o lock de escrita.
- **Correção:** banco corrompido é erro do backend; o esquema só é criado em banco novo e índice de esquema mais novo é recusado (7e81169).

## E-003 — Flatpak: apps legados, catálogo vazio, ranking por título
- **Quando:** 2026-11-20, P2.
- **Sintoma:** `org.mozilla.firefox` e outros 18 apps nunca indexados; catálogo vazio ou não XML contava como sucesso; Firefox do Flatpak ficava abaixo de centenas de `firefox-*` do AUR.
- **Causa:** só se aceitava `type="component"`; ausência de validação; título exato não pesava como nome exato.
- **Correção:** aceitar `type="desktop"`, catálogo vazio vira erro, app instalado em qualquer branch, título exato ranqueia como nome (a9e1cce).

## E-004 — Sondas `pkexec` do agente abriram prompts de senha
- **Quando:** 2026-11-20, P3.
- **Sintoma:** `pkexec /usr/bin/true` e `/usr/bin/id` rodados pelo agente mostraram diálogos de senha admin ao usuário.
- **Causa:** suposição de que nenhum agente gráfico de polkit estava ativo; no niri ele sobe sob demanda.
- **Correção:** regra permanente: agentes não rodam `pkexec`, `sudo` nem `store install|remove|upgrade`; só sondas de leitura (`pkcheck`, `pkaction`). Memória `no-surprise-auth-prompts` e `AGENTS.md`.

## E-005 — umask do chamador vazando para a libalpm
- **Quando:** 2026-11-20, P3 (revisão de segurança).
- **Sintoma:** pacotes baixados ficavam em cache com modo 0666.
- **Causa:** o helper herdava a umask do processo que o chamou.
- **Correção:** fixar umask, `setsid`, ignorar sinais de controle de job, resetar limites de recursos; hooks pré-transação e commit concluído passam a ser o ponto sem volta do cancelamento; nomes e proteção só decididos sob o lock (3ed0cb2).

## E-006 — Cliente parado no stdout travava a transação
- **Quando:** 2026-11-20, P3.
- **Sintoma:** um cliente que parava de ler eventos podia bloquear uma transação segurando o lock do banco.
- **Causa:** eventos escritos na thread da transação.
- **Correção:** escrita por thread separada; progresso é descartado se ela atrasa, outros eventos nunca; o aborto escreve com prazo (8bc1bf9). Também: recusar install com bancos à frente do sistema (atualização parcial), apagar pacote corrompido do cache, mensagem para chave não confiável.

## E-007 — Texto de repositório e do helper chegando ao terminal
- **Quando:** 2026-11-20, P3.
- **Sintoma:** caracteres de controle em nomes, versões, descrições ou mensagens iriam cru ao terminal; GUI longeva acumularia zumbis do helper.
- **Causa:** saída sem sanitização; filhos abandonados não eram coletados.
- **Correção:** sanitização na CLI, progresso a cada 10%, `pkexec` por caminho absoluto, thread coletora de filhos (7e3fad0).

## E-008 — `dev-install.sh` confiava em arquivos graváveis pelo usuário
- **Quando:** 2026-11-20, P3.
- **Sintoma:** o script rodava como root instalando arquivos que o usuário podia trocar no meio.
- **Causa:** leitura e instalação em duas etapas, seguindo symlinks.
- **Correção:** recusar symlinks, ler cada entrada uma vez com abertura sem seguir links para diretório privado, verificar a cópia (magic ELF, ação polkit) e instalar a cópia (1b06b18).

## E-009 — `manual-check.sh` recusava rodar sem explicar
- **Quando:** 2026-11-21, P3.
- **Sintoma:** diálogo de senha cancelado no passo 3 deixou `tree` e `cowsay` instalados; a execução seguinte parou sem dizer por quê.
- **Causa:** guarda silenciosa; saída antecipada sem relatório.
- **Correção:** a guarda mostra a data da instalação e o comando de limpeza; saída antecipada lista o que ficou instalado. Cada execução de `store` é um novo sujeito polkit, o que também foi corrigido no script (041fd8d, 3ed0cb2).

## E-010 — `git push` na `main` recusado (GH006)
- **Quando:** 2026-11-21, P3.
- **Sintoma:** "Protected branch update failed… 3 of 3 required status checks are expected".
- **Causa:** a proteção da `main` exige PR com os três checks.
- **Correção:** fluxo branch → PR → CI verde → merge sem squash → `git pull --rebase`.

## E-011 — Consultas de 1 a 2 letras ranqueavam errado
- **Quando:** 2026-11-21, P4.
- **Sintoma:** apps Flatpak passavam à frente de nomes mais curtos; "Qt Creator" soterrado pelo id longo.
- **Causa:** pacote sem título ficava `NULL` contra `0` na chave de match exato e ordenava depois de todo pacote com título; sem peso pelo tamanho do título.
- **Correção:** tratar `NULL` como ausente e ranquear consultas curtas pelo tamanho do título (1a1d5d8).
