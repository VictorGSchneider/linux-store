# Helper privilegiado (pacman) — Design

Data: 2026-11-20 · Estado: rascunho para aprovação
Complementa `docs/2026-11-18-linux-store-design.md` (etapa 4 das "Etapas").

## Intenção e porquê

A GUI nunca roda como root, mas instalar, remover e atualizar pacotes do pacman exige root. O helper é o **único código privilegiado** do projeto: pequeno, sem interface, e feito para ser auditado. Quem instala software a partir de uma loja precisa poder confiar que ela não vira uma porta para o root.

## Decisões aceitas (suas)

1. **Processo curto via `pkexec`**: o app chama `pkexec` num binário helper que lê um pedido, faz o trabalho e termina. Nada fica rodando como root entre operações. O modo serviço D-Bus fica para depois, se a GUI precisar de cancelamento limpo ou progresso mais rico.
2. **Cache de senha com `auth_admin_keep`**, com o **processo da GUI chamando o `pkexec` diretamente** (ele é o "sujeito" da autorização).
3. **libalpm**, não o comando `pacman`: nada de interpretar texto de CLI.
4. **Operações em lista**: marcar vários programas e confirmar uma vez.
5. **Interface `Installer`** no `core`, para o PackageKit entrar depois como adaptador de Debian e Fedora.
6. **Escopo v1**: instalar e remover listas de pacotes dos repositórios, atualizar o sistema, com progresso por eventos. Fora: instalar pacotes do AUR já compilados (plano seguinte) e o Flatpak (ele já tem o `flatpak-system-helper` com polkit próprio).

## O que foi medido nesta máquina (CachyOS, polkit 128, pacman 7.1)

- `pkexec` custa cerca de 15 a 20 ms por operação (2,8 ms para iniciar e 12,6 ms de ida e volta ao `polkitd`). O custo é desprezível.
- A ação padrão do `pkexec` é `auth_admin` **sem cache**. Com uma ação própria em `auth_admin_keep` e `exec.path`, a segunda chamada do mesmo processo levou **0,03 s sem pedir senha**; uma chamada vinda de **outro processo** pediu a senha de novo. O cache vale por processo, não por usuário nem por sessão.
- Nenhum agente gráfico de autenticação roda hoje no niri (só o `polkitd`). `pkexec` sem terminal cai num agente de texto que só funciona em terminal.
- O **PackageKit foi testado e descartado como caminho único para o pacman**: o backend `alpm` dele recusa pacotes `x86_64_v3` do CachyOS (`package architecture is not valid`), reporta atualizações falsas e leva 3 a 4 s em `GetUpdates`. A libalpm configurada via `pacmanconf` planejou os mesmos pacotes `x86_64_v3` sem erro (em cópia dos bancos, sem commit).

## Objetivos

- Instalar, remover e atualizar pacotes dos repositórios configurados, com uma senha por ~5 minutos.
- Um pedido é uma **transação única**: ou tudo é aplicado ou nada.
- Progresso por eventos, para a GUI mostrar barra e etapa.
- Funcionar no CachyOS (repositórios `x86_64_v3`) e no Arch.

## Não-objetivos (v1)

- Instalar arquivos de pacote escolhidos pelo usuário (AUR compilado): plano seguinte.
- Operação de "sincronizar bancos" isolada (veja a decisão 1 abaixo).
- Cancelar no meio do `commit` da transação.
- Mais de uma distro: o helper é só do pacman.

## Anti-objetivos (seria falha mesmo funcionando)

- O helper aceitar **caminhos, URLs, nomes de repositório ou opções** vindos do pedido.
- O helper chamar o comando `pacman` ou qualquer shell.
- Qualquer opção, variável de ambiente ou flag que mude `--root`, `--dbpath` ou o `pacman.conf` na versão de produção.
- Deixar o banco travado (`db.lck`) ou uma transação pela metade.
- Baixar a exigência de assinatura dos pacotes.
- A GUI rodar como root.

## Modelo de ameaças

**O que protegemos:** o root da máquina e a integridade do banco de pacotes.

**Quem ataca:** qualquer processo sem privilégio do mesmo usuário (malware na sessão, uma extensão maliciosa) ou um processo da GUI comprometido. Fora do alcance: um atacante que já é root, repositórios ou mirrors comprometidos (a verificação de assinatura da libalpm continua valendo) e acesso físico.

**A ameaça principal é o "confused deputy"**: um processo sem privilégio usa o helper, que tem privilégio, para fazer o que ele mesmo não poderia. Cada mitigação abaixo trata disso.

| Ameaça | Mitigação |
|---|---|
| Pedido forjado pede algo perigoso | O pedido só tem uma **operação** (enum) e uma **lista de nomes de pacote**. Nomes seguem `^[a-z0-9@._+][a-z0-9@._+-]*$` (sem `/`, sem `-` inicial, sem espaço), no máximo 200 por pedido, sem duplicatas. Campos desconhecidos são rejeitados. Tamanho máximo do pedido: 64 KiB. |
| Injeção de opção ou caminho | O helper nunca monta um comando nem passa texto do usuário a um shell. Nomes são só resolvidos nos bancos do sistema. |
| Instalar algo que não está nos repositórios | O pedido só nomeia pacotes; quem resolve é o helper, nos bancos sincronizados e no `pacman.conf` (ambos de root). O pior caso é equivalente a um administrador rodar `pacman -S`. |
| Pacote malicioso ou adulterado | Vale o `SigLevel` do `pacman.conf`. O helper nunca o relaxa. |
| Mudar o ambiente do helper | O `pkexec` limpa o ambiente. O helper ignora `HOME`, `PATH` do usuário e variáveis, e lê só `/etc/pacman.conf`. |
| Downloads rodando como root | Se o `pacman.conf` define `DownloadUser` (aqui, `alpm`), o `alpm_with_conf` já aplica o sandbox de download. É obrigatório manter isso. |
| Remover algo essencial | A libalpm já recusa remover o que quebra dependências (testado: remover o `glibc` lista os dependentes). Além disso, o helper recusa qualquer remoção que toque o pacote `pacman`, o metapacote `base` (e o que só ele exige) e o próprio helper. |
| Cache de 5 minutos: depois de uma senha, o processo da GUI roda operações sem perguntar | Risco **aceito** e documentado. É limitado ao processo da GUI (medido: outro processo pede senha de novo), e a validação acima vale em toda chamada. |
| Transação pela metade ou banco travado | Uma única transação da libalpm por pedido. O helper trata `SIGTERM` e `SIGINT` liberando a transação, nunca apaga um `db.lck` sozinho e, se o banco estiver travado, recusa com mensagem clara. |
| Perguntas da libalpm (conflitos, provedores ambíguos) | Na v1, qualquer pergunta que exigiria escolha aborta a transação com a lista de opções no erro. Nunca responde "sim" por padrão. |
| Falta de rastro | Cada operação é registrada no journal: usuário que chamou (`PKEXEC_UID`), operação e nomes. |
| Helper comprometido pelo arquivo de política | O binário fica em `/usr/lib/linux-store/store-helper` (`root:root`, `0755`), o único caminho de `exec.path`. |

## Protocolo (helper ⇄ GUI)

- A GUI executa `pkexec /usr/lib/linux-store/store-helper` **sem argumentos**.
- **Pedido** (uma linha JSON no `stdin`): `{"op":"install","packages":["tree","sl"]}` com `op` em `install`, `remove`, `upgrade` (que não tem `packages`). Uma segunda linha `{"op":"cancel"}` é aceita antes do commit.
- **Eventos** (uma linha JSON por linha no `stdout`): `started`, `progress` (etapa, pacote, percentual), `done` (lista do que mudou) e `error` (código e mensagem). O `stderr` leva só diagnóstico.
- **Códigos de saída**: 0 sucesso; 1 erro de operação; 2 pedido inválido; do `pkexec`, 126 = o usuário dispensou a autenticação e 127 = não autorizado ou falha ao autenticar.
- Os tipos do pedido e dos eventos ficam em um crate compartilhado (`store-protocol`), usado pelo helper e pelo cliente, para os dois lados não divergirem.

## Cancelar

O usuário não consegue enviar sinais a um processo root, então o cancelamento é **no protocolo**: a GUI manda `cancel` ou fecha o `stdin`. O helper para durante o download e antes do commit. **Depois que o commit começa, ele termina** (nunca deixa a instalação pela metade) e responde `too_late`.

## Políticas do polkit

O `pkexec` associa a ação ao **caminho do binário**, então a v1 tem **uma única ação**, `org.linuxstore.helper.run`, com `auth_admin_keep` em `allow_active` e `auth_admin` nos demais, e `exec.path` apontando para o helper. A consequência é que a política não distingue instalar de remover; um administrador que queira isso precisaria de um modo D-Bus, com uma ação por operação.

## Decisões minhas, a confirmar

1. **Não existe a operação "sincronizar bancos" isolada.** O escopo aceito citava "sincronizar bancos"; mantenho essa capacidade só **dentro do `upgrade`** (equivale a `pacman -Syu`). Sincronizar sozinho (`-Sy`) sem atualizar é o caminho clássico para quebrar o Arch por atualização parcial. Instalar e remover usam os bancos como estão. Como a GUI mostra um catálogo atualizado sem esse comando é assunto de um plano próprio (a ideia é um banco temporário sem privilégio, como faz o `checkupdates`).
2. **Uma ação do polkit para tudo**, pela limitação do `pkexec` acima.
3. **Lista de proteção na remoção**: `pacman`, o metapacote `base` e o helper.
4. **A interface `Installer`** tem `install`, `remove` e `upgrade`, cada uma devolvendo um fluxo de eventos; o cliente de `pkexec` é a única implementação por enquanto.

## Alterações depois da revisão de segurança

A revisão independente do código achou problemas que mudaram o desenho (cada um tem teste):

- **Umask:** o `pkexec` repassa a umask de quem chamou. Com umask 0 a libalpm deixava os pacotes baixados com modo `0666` (donos: root), que qualquer processo do usuário podia reescrever entre a checagem de assinatura e a extração. O helper agora fixa `umask 022` antes de tudo.
- **Higiene do processo:** o helper sai da sessão do terminal (`setsid`), ignora `SIGQUIT`, `SIGTSTP`, `SIGTTIN`, `SIGTTOU` e `SIGXFSZ`, e restaura os limites de recursos (tamanho de arquivo, CPU, memória, descritores). O terminal de quem chamou não pode mais parar nem matar a transação no meio.
- **Hooks são ponto sem volta:** os hooks de pré-transação do pacman (snapshots, remoção de imagens do kernel) rodam antes da primeira operação de pacote e já alteram o sistema. O ponto sem volta do cancelamento agora é o início dos hooks (evento `hooks` no protocolo), não a primeira operação de pacote.
- **Janela fecha quando o commit termina:** depois que `trans_commit` retorna (com sucesso ou erro) um cancelamento é tarde demais, para nunca remover um `db.lck` que não é mais nosso.
- **`upgrade` não cancela depois do refresh:** atualizar os bancos sem atualizar o sistema é o estado de atualização parcial que o design proíbe. Depois do refresh o `upgrade` vai até o fim.
- **Decisões sob o lock:** nomes são resolvidos e a proteção é decidida **depois** de tomar o lock, porque a libalpm não recarrega seu cache de bancos.
- **Testes não escrevem no journal do sistema.**

Depois disso, os itens menores da revisão também foram tratados (cada um com teste):

- **Escrita de eventos não bloqueia:** uma thread própria escreve a saída. Um cliente que parou de ler não segura mais uma transação que mantém o lock; eventos de progresso são descartados quando a fila enche, os demais nunca. O caminho de aborto escreve com limite de tempo e não espera essa thread.
- **Auditoria com resultado:** além da linha do pedido, o journal recebe `result=ok changes=N` ou `result=error code=...`.
- **`install` recusa atualização parcial:** com os bancos do pacman à frente do sistema (por exemplo depois de um `pacman -Sy` solto), o pedido falha com `upgrade_needed` e manda rodar o `upgrade` antes. Pacotes em `IgnorePkg` não contam.
- **Pacote corrompido no cache é apagado**, para a próxima tentativa baixar de novo, e uma chave de assinatura não confiável gera uma mensagem explicando o que fazer.
- **Saída da CLI:** nenhum caractere de controle vindo de repositórios ou do helper chega ao terminal, o progresso sai a cada 10% e os passos de verificação sem nome de pacote não aparecem mais.
- **Cliente:** chama o `pkexec` pelo caminho absoluto e entrega um helper que não sai a uma thread que o recolhe, sem acumular zumbis.
- **`dev-install.sh`:** recusa symlinks, lê cada arquivo uma vez com abertura sem seguir links para um diretório privado de root, confere a **cópia** (ELF e a ação do polkit) e instala essa cópia.

## Estrutura proposta

- `protocol/` (`store-protocol`): tipos do pedido e dos eventos, com a validação.
- `helper/` (`store-helper`): binário e biblioteca com as operações sobre a libalpm. Dependências mínimas: `alpm`, `alpm-utils`, `pacmanconf`, `serde_json`.
- `installers/pkexec/` (`store-installer-pkexec`): cliente que inicia o `pkexec`, escreve o pedido e lê os eventos.
- `core/`: o trait `Installer` e os eventos.
- `packaging/`: o arquivo de política do polkit e um script de instalação para desenvolvimento (`install-dev` e `uninstall-dev`, que você roda com `sudo`).

## Como testar

- **Validação e protocolo**: testes unitários, com entradas hostis (nomes com `/`, `-x`, espaços, Unicode, 10 mil nomes, JSON gigante, campos extras).
- **Operações da libalpm**: sem root e sem tocar no sistema, numa raiz temporária com bancos e pacotes de fixture servidos por `file://`. Isso exercita instalar, remover, atualizar, conflitos, dependência quebrada e transação interrompida.
- Os parâmetros `--root`/`--dbpath` existem **só** no código de teste (feature separada), nunca no binário de produção.
- Fora do CI: um roteiro manual, com a sua senha, para o fluxo completo via `pkexec`.

## Em aberto, para quem constrói

- Confirmar se o `pkexec` faz `exec` do helper ou `fork` (afeta sinais e a detecção de saída).
- Como a GUI detecta a falta de um agente de autenticação e orienta o usuário (códigos 126 e 127 não bastam sozinhos).
- Lista exata de pacotes protegidos na remoção e as mensagens de erro.
- Como gravar no journal sem dependência pesada.
