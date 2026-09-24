# HD Cleaner — Status do projeto

**Atualizado em 24/09/2026**, ao final da Fase 12 (relatório HTML, linha do tempo e CLI).

Este arquivo registra **tudo o que já foi feito** e **tudo o que ainda falta**, seguindo as seções da especificação original. Nenhuma funcionalidade listada aqui como feita é simulada: todas foram compiladas, testadas por testes automatizados e, quando indicado, verificadas no app real, nesta máquina.

Documentos relacionados:

- `docs/ARCHITECTURE.md` — arquitetura, modelo de segurança e tabela de estado resumida
- `README.md` — início rápido e exemplos da CLI

---

## 1. Visão geral

| Item | Situação |
|---|---|
| Stack | Rust 1.98 (MSVC) + Tauri 2 + React 19 + TypeScript + Vite |
| Estrutura | `crates/hdcleaner-core` (toda a lógica) · `crates/hdcleaner-cli` (binário `hdcleaner`) · `crates/test-fixtures` (programa falso de teste) · `src-tauri` (camada de comandos) · `src` (interface) |
| Testes automatizados | **96 testes Rust + 7 testes de ponta a ponta (desinstalação normal, forçada, inicialização, árvore de processos, limpeza em sandbox, Lixeira e monitor de instalação) passando**. Os testes destrutivos usam apenas pastas temporárias ou o programa falso de teste (sob o perfil do usuário) |
| Tipagem do frontend | `tsc --noEmit` sem erros |
| Build de release | `hd-cleaner.exe` com 13,4 MB (sem instalador). Instaladores NSIS/MSI ainda não foram gerados |
| Controle de versão | A pasta **não é um repositório git** e nenhum commit foi feito |
| Idiomas | pt-BR e inglês, com todas as chaves presentes nos dois idiomas (checado pelo compilador TypeScript) |

### Como executar

```powershell
npm install
npx tauri dev                          # app em modo desenvolvimento
cargo test --workspace                 # testes
cargo build --release -p hdcleaner-cli     # CLI: target\release\hdcleaner.exe
npx tauri build --no-bundle            # exe de release (sem instalador)
```

O Rust está em `%USERPROFILE%\.cargo\bin`, que não está no PATH por padrão.

Os dados locais ficam em `%LOCALAPPDATA%\HDCleaner`:

- `hdcleaner.db` — banco SQLite
- `logs\` — logs internos
- `snapshots\` — snapshots das varreduras

---

## 2. O que foi feito

### Fase 1 — Base

- Workspace Cargo com três crates e o app Tauri 2.
- O nome do produto fica centralizado em três lugares:
  - `crates/hdcleaner-core/src/branding.rs`
  - `src/config/branding.ts`
  - `src-tauri/tauri.conf.json`
- Ícone e logo originais: um anel de disco com um segmento de uso em verde-água.
- Navegação por barra lateral:
  - Os grupos são Armazenamento, Aplicativos e Sistema.
  - As páginas ainda não implementadas levam uma marcação "—" e abrem uma tela explícita de "Ainda não implementado" que descreve o que está planejado.
- Tema escuro (padrão), tema claro e opção "sistema"; escala de fonte de 90% a 130%. Os tamanhos da interface usam `rem`, o que funciona bem em telas de alta resolução (DPI alto).
- Banco SQLite com migrações. Tabelas:
  - `settings`
  - `operations` — histórico, que também funciona como *journal*
  - `scans` — índice dos snapshots
- Logs com `tracing` em arquivo diário. Os logs de depuração podem ser ativados nas configurações.
- Tipo de erro estruturado. Cada erro informa:
  - o tipo;
  - a mensagem;
  - o caminho envolvido;
  - o código do Win32;
  - uma sugestão de ação ("executar como administrador", "analisar novamente" etc.).

  A interface mostra o erro de forma compreensível, sem mensagens genéricas.
- Configurações persistidas, validadas por uma lista branca de chaves no backend.
- Página **Ferramentas do Windows**:
  - Atalhos para 14 ferramentas: Gerenciador de Tarefas, Serviços, Regedit, Gerenciador de Dispositivos, Gerenciamento de Disco, Visualizador de Eventos, msinfo32, Monitor de Recursos, Painel de Controle, Configurações, Propriedades do Sistema, Variáveis de Ambiente, PowerShell e cmd.
  - O backend recebe apenas o id da ferramenta (lista branca) e usa `ShellExecuteExW`, de modo que o UAC aparece quando a ferramenta exige.
- Botão **Reiniciar como administrador**, que o usuário aciona explicitamente.

### Fase 2 — Scanner de disco

- **Enumeração de unidades.** Para cada unidade: letra, rótulo, sistema de arquivos, tipo (fixa, removível, rede, óptica, RAM), SSD/HDD (via `IOCTL_STORAGE_QUERY_PROPERTY`, sem precisar de admin), capacidade, espaço livre, espaço usado e se aceita a varredura rápida.
- **Varredura padrão** (`WindowsApiScanner`):
  - Usa `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)` em paralelo com rayon, com *fallback* para `FindFirstFileExW(LARGE_FETCH)`.
  - Funciona em NTFS sem admin, FAT, FAT32, exFAT, ReFS, rede e pastas avulsas.
  - Suporta caminhos com mais de 260 caracteres (prefixo `\\?\`).
  - Junções e links simbólicos **não são seguidos** por padrão (configurável). Placeholders do OneDrive são detectados.
  - **Hard links são contados uma única vez.**
  - Pastas ilegíveis são contadas e listadas, sem esconder o erro.
- **Varredura rápida NTFS** (`NtfsFastScanner`) — lê a Master File Table (MFT) diretamente:
  - Obtém a geometria com `FSCTL_GET_NTFS_VOLUME_DATA` e lê o runlist do próprio `$MFT`.
  - Faz a leitura sequencial numa thread separada e o *parsing* em paralelo.
  - Trata fixups, registros de extensão, hard links, nomes 8.3 (ocultos), arquivos sparse e comprimidos.
  - O espaço alocado é calculado **pelos clusters reais do runlist**. Isso corrigiu o `$BadClus`, que aparecia com 473 GB fantasmas.
  - **Validado no C:\ real com UAC:** 1,26 milhão de entradas em **3,5 s**. O total de 345 GB bate com o espaço usado segundo o Windows (345,5 GB).
- **Seleção automática do método:**
  - Raiz NTFS com privilégio → MFT.
  - Qualquer outro caso → varredura padrão. O motivo do *fallback* aparece na interface.
- **Elevação com privilégio mínimo:**
  - O app roda sem admin. Para a varredura MFT, o próprio exe é relançado via UAC em modo *helper*.
  - O *helper* aceita **somente** `scan-ntfs <letra> <canal>`, com validação estrita.
  - O resultado volta por um *named pipe* criado pelo processo não elevado, com `FILE_FLAG_FIRST_PIPE_INSTANCE` e `PIPE_REJECT_REMOTE_CLIENTS`.
  - Não existe API genérica do tipo "executar como admin".
- **Progresso ao vivo:**
  - Mostra fase, arquivos, pastas, bytes, itens/s, tempo decorrido e caminho atual.
  - O cancelamento é real.
- **Modelo de memória compacto:**
  - 76 bytes por nó.
  - Todos os nomes num único *pool* UTF-8.
  - Filhos em formato CSR, pré-ordenados por tamanho.
  - Extensões internadas.
- Diferencia **tamanho lógico** de **tamanho alocado**.
- **Tree view virtualizada:**
  - Colunas: nome, barra de %, %, alocado, tamanho, arquivos, pastas, modificado.
  - Ordenação por coluna, expandir/recolher com teclado, Expandir tudo e Recolher tudo.
  - O estado (pastas expandidas, seleção, raiz do mapa) é **preservado no rescan (F5)**.
- **Medido nesta máquina (C:\ sem admin):** cerca de 1,18 milhão de entradas em 6,5 a 11,7 s.

### Fase 3 — Treemap

- Layout *squarified* calculado em Rust e enviado em formato binário (24 bytes por retângulo). O desenho é feito em Canvas.
- Cores por categoria: vídeos, imagens, áudio, executáveis, compactados, jogos, documentos, desenvolvimento, cache e logs, sistema, instaladores e imagens de disco. Há uma legenda.
- **Hover** com *tooltip*: nome, tamanho alocado e lógico, extensão, data e caminho. Um índice espacial evita redesenhar o mapa inteiro a cada movimento do mouse.
- **Clique:**
  - Um clique seleciona o item e o revela na árvore.
  - Duplo clique numa pasta dá zoom nela.
  - Duplo clique num arquivo leva até ele na árvore.
- Trilha de navegação (*breadcrumb*) com botão para subir um nível.
- Menu de contexto completo e **destaque** de nós (usado pelo App Storage Map).
- Divisor redimensionável entre a tabela e o mapa; a altura escolhida fica salva.

### Fase 4 — Busca, Arquivos Grandes e Duplicados

- **Linguagem de busca:**
  - nome e curingas (`*.mp4`, `cache*`);
  - tamanho (`size:>1GB`, `size:500MB..5GB`);
  - datas (`modified:<30d`, `modified:today`, `modified:2024-01-01..2024-06-30`, e o mesmo para `created:` e `accessed:`);
  - extensão (`ext:mp4,mkv`);
  - caminho (`path:"AppData"`);
  - tipo (`type:file`, `type:dir`, `type:video`…);
  - negação com `-` ou `!`.

  Os filtros podem ser combinados. Medido: **15 ms** em 1,2 milhão de nós.
- Filtros rápidos: >100 MB, >1 GB, hoje, esta semana, vídeos, imagens, compactados, instaladores e executáveis.
- File view de resultados com as colunas nome, caminho, tamanho, alocado, extensão, criado, modificado, acessado e atributos. Qualquer coluna pode ser usada para ordenar.
- **Arquivos Grandes:**
  - Top 100, 500, 1000, 5000 ou um número personalizado.
  - Agrupamento por extensão, pasta ou categoria.
  - Histograma de distribuição de tamanhos e exportação CSV.
- **Duplicados:**
  - Pipeline: tamanho → hash parcial BLAKE3 (início e fim do arquivo) → hash completo apenas dos candidatos restantes.
  - Também há os modos rápidos: nome+tamanho e nome+tamanho+data.
  - Arquivos da nuvem nunca são baixados.
  - Seleção inteligente: manter o mais antigo, o mais novo, o de caminho mais curto ou o que está numa pasta escolhida. Ela **apenas pré-seleciona**.
  - A interface **impede apagar todas as cópias de um grupo**.
  - Medido no perfil do usuário (≥ 20 MB): 33 grupos, 2,82 GB desperdiçados, cerca de 6 s.

### Fase 5 — Programas instalados

- **Fontes:**
  - Chaves Uninstall do HKLM 64-bit, do HKLM 32-bit (WOW6432Node) e do HKCU.
  - MSI, identificado por `WindowsInstaller=1` e ProductCode.
  - AppX, MSIX e Microsoft Store via o `PackageManager` do WinRT, sem precisar de admin.
- **Campos de cada programa:**
  - ícone, nome, versão, fabricante e data de instalação;
  - local de instalação (registrado ou inferido pelo desinstalador ou ícone);
  - arquitetura e escopo (máquina ou usuário);
  - comandos de desinstalação normal, silenciosa e de modificação;
  - tamanho informado pelo instalador e tamanho real;
  - site e chave do Registro;
  - ProductCode do MSI, nome completo e família do pacote, tipo de assinatura e dependências.
- Componentes de sistema, atualizações/KBs, entradas filhas e frameworks ficam **ocultos por padrão** e podem ser exibidos com um botão.
- Busca instantânea, filtro por origem e ordenação por nome, fabricante, versão, data, tamanho informado ou tamanho real.
- **Ícones:**
  - Ordem de tentativa: DisplayIcon → desinstalador → `.ico` da pasta de instalação → `.exe` com nome parecido.
  - O ícone é extraído como recurso, **sem executar o arquivo**, e sempre recodificado em PNG.
  - Resultado: 101 de 116 programas com ícone.
- **Tamanho real (TRUE APP SIZE):**
  - Soma a pasta de instalação com AppData Local, AppData Roaming, ProgramData, `LocalAppData\Programs` e a pasta de dados do pacote.
  - Separa o total em programa, dados do usuário, cache e logs.
  - Cada local recebe um nível de confiança — **Confirmado**, **Provável** ou **Possivelmente relacionado** — e o motivo.
  - Locais "possivelmente relacionados" ficam **fora do total**.
  - Se já houver uma varredura carregada, o tamanho sai dela instantaneamente.
  - Exemplo real: o Discord declara 138 MB, mas usa 591 MB confirmados, mais 433 MB possivelmente relacionados.
- **Integração com o analisador:**
  - "Ver no mapa do disco" destaca as pastas do programa no treemap e varre a unidade antes, se necessário.
  - "Identificar programa instalado" aparece no menu de contexto do analisador.
  - O painel de detalhes mostra o aplicativo relacionado, com link para a página Programas.
- O Dashboard mostra a contagem de programas instalados.
- CLI: `hdcleaner programs [filtro] [--sizes] [--all] [--json]`.

### Fase 6 — Desinstalação normal e busca de sobras

- **Assistente de desinstalação** (botão "Desinstalar" na página Programas):
  1. confirmação mostrando **o comando exato** que será executado;
  2. ponto de restauração opcional (`SRSetRestorePointW` via helper elevado, com a opção "continuar sem" se falhar);
  3. execução do desinstalador oficial;
  4. espera pela **árvore inteira de processos** (Inno/NSIS copiam-se para o Temp e saem logo);
  5. verificação se o programa **ainda está instalado** — se o usuário cancelou, a busca de sobras é bloqueada;
  6. busca de sobras;
  7. seleção pelo usuário;
  8. backup e remoção;
  9. resultado por item;
  10. registro no histórico com journal (operação "running" antes de começar).
- **Execução segura do desinstalador:**
  - MSI → `System32\msiexec.exe /x {ProductCode}` montado a partir de um GUID validado (a string do Registro não é usada).
  - Demais → exe e parâmetros separados, via `ShellExecuteExW` (o UAC aparece se o manifesto exigir; nada de `cmd.exe`).
  - Store/AppX → `PackageManager.RemovePackageWithOptionsAsync` (pacotes de sistema recusados).
  - Modo silencioso usa `QuietUninstallString` ou `msiexec /passive`.
- **Busca de sobras** (`leftovers.rs`) em três níveis:
  - **Seguro:** pasta registrada, entrada Uninstall remanescente, dados do pacote; atalhos (alvo resolvido via `IShellLinkW`), entradas Run, serviços, tarefas agendadas e App Paths **cujo alvo está dentro da pasta do programa**.
  - **Moderado:** pastas em AppData/ProgramData com o nome do programa, `Software\Fabricante\Programa`, `Software\Programa`, pasta no Menu Iniciar, atalhos/entradas/serviços/tarefas quebrados com o nome do programa.
  - **Avançado:** chave do fabricante que só continha este produto, pastas cujo nome *contém* o do programa (inclusive no Temp).
- Cada item tem confiança de 0 a 100, motivo, evidência (ex.: alvo do atalho), tamanho, risco da Protection Engine e a marca "compartilhado".
  - Só vem pré-marcado o que tem **≥ 70% e não é compartilhado**.
  - Pastas de **outros programas instalados** nunca são propostas.
  - Locais bloqueados nunca aparecem.
- **Remoção com backup:**
  - Itens do Registro são exportados para um `.reg` (REGEDIT5, UTF-16) **antes** de qualquer exclusão; sem backup, nada do Registro é tocado.
  - Arquivos vão para a Lixeira (opcional).
  - Há dry run.
  - Itens em locais sensíveis exigem confirmação explícita.
  - Um `manifest.json` é gravado na pasta do backup.
- **Allowlist do Registro** (`regops.rs`): só se exclui `Software\<Fabricante>\…`, entradas `Uninstall\<chave>` e `App Paths\<exe>`, e valores de `Run`/`RunOnce`. Windows, Classes, Policies, SYSTEM etc. são recusados. Chaves de fabricantes compartilhados (Google, Mozilla…) só podem ser removidas nas subchaves de produto.
- **Elevação:** o que precisa de admin (HKLM, Program Files, serviços, tarefas) é agrupado em **um único UAC**.
  - Novo comando do helper `apply-ops` (pipe duplex), com operações tipadas: excluir caminho, excluir chave/valor, excluir serviço, excluir tarefa, criar ponto de restauração.
  - Cada operação é revalidada **dentro** do helper: Protection Engine, allowlist, serviço cujo ImagePath precisa bater com o revisado e que não esteja na pasta do Windows, tarefas fora de `\Microsoft\`.
- **CLI:** `hdcleaner uninstall "Nome" [--quiet] [--level safe|moderate|advanced] [--confirm] [--remove-leftovers] [--dry-run]`. Nada roda sem `--confirm`.
- **Configurações → Desinstalador:** nível padrão da busca de sobras e ponto de restauração por padrão. O backup do Registro fica sempre ativo.
- **Programa falso de teste** (`hdcleaner-fake-app`, seção 60):
  - instala-se (pasta, AppData com cache e logs, chave do fabricante, entrada Uninstall, atalho no Menu Iniciar, entrada Run);
  - tem um desinstalador "desleixado" que deixa sobras de propósito.
- **Teste de ponta a ponta:** instala, desinstala, verifica os três níveis, faz dry run, remove, confere o backup `.reg` e confirma que uma nova busca não encontra nada.
- **Verificado no app real** (pelo assistente) e na CLI: o programa sumiu da lista, 6 sobras foram removidas, a chave do fabricante (50%, Avançado) foi mantida por não vir marcada, e o histórico registrou `uninstall completed` com o caminho do backup.

### Fase 7 — Desinstalação forçada, em lote e rápida

- **Desinstalação forçada** (botão "Desinstalação forçada…" na barra da página Programas, ou no painel de detalhes, já preenchido):
  - O usuário informa o que sabe: nome, `.exe` e/ou pasta (com diálogos de escolha). A pasta é deduzida do `.exe` se não for informada.
  - `forced.rs` valida a entrada: pastas BLOCKED da Protection Engine são recusadas; nome com menos de 3 letras sem pasta é recusado; pasta inexistente + nome continua pela busca por nome (caso comum de programa quebrado).
  - Lê as informações de versão do `.exe` (fabricante, produto, versão) para completar nome e fabricante.
  - Lista os **programas registrados que provavelmente são o mesmo produto** (mesma pasta, desinstalador dentro da pasta, mesmo nome) — o usuário escolhe um deles ou "não registrado".
  - Se o programa escolhido ainda tem desinstalador, oferece rodá-lo antes; se o desinstalador sumiu, mostra o motivo e segue direto para as sobras.
  - Mostra os **processos rodando da pasta** com botão "Encerrar". O encerramento revalida o caminho da imagem do processo (se o PID foi reutilizado, recusa), recusa PIDs de sistema, o próprio app e processos da pasta do Windows, e fica no histórico (`end-process`).
  - A busca de sobras roda sem a checagem "ainda instalado" e trata a pasta informada como "pasta escolhida pelo usuário" (90%). A entrada Uninstall órfã é proposta como sobra.
  - Histórico: `forced-uninstall`.
- **Desinstalação em lote** (caixas de seleção na lista + "Desinstalar selecionados (N)"):
  - Prepara todos, mostra a lista e as opções (preferir modo silencioso, remoção automática, ponto de restauração **único**, nível).
  - Fila **sequencial** — nunca dois desinstaladores ao mesmo tempo — com status por programa (aguardando, desinstalando, desinstalado, ainda instalado, falhou).
  - Depois busca as sobras de todos e mostra a revisão **agrupada por programa**; a remoção usa **um único UAC** para todos (`leftovers_remove_batch`), com backup separado por programa.
  - A seleção é limpa automaticamente quando os programas somem da lista.
- **Desinstalação rápida** (seção 22; botão no painel de detalhes e opção do lote):
  - Remove automaticamente **só** o que é inequívoco: nível Seguro, confiança ≥ 90%, não compartilhado, fora de locais sensíveis. Todo o resto é mostrado para confirmação (ou "Concluir sem remover").
- **Programa falso de teste:** ganhou `--tray` (processo residente) e `break` (apaga o desinstalador, simulando instalação quebrada).
- **Teste de ponta a ponta `forced_flow`:** instala, quebra, inicia o processo, resolve pela pasta, encerra o processo, busca sobras em modo forçado, remove e confirma que tudo sumiu.
- **Verificado no app real:**
  - Lote com 2 programas em modo rápido: ambos desinstalados, 6 sobras removidas automaticamente, 6 revisadas e removidas, um backup por programa.
  - Forçada num programa sem desinstalador e com processo rodando: casou pela pasta, encerrou o processo, 7 sobras removidas (inclusive a entrada Uninstall órfã).
  - Rápida: 3 sobras inequívocas removidas automaticamente; as 3 por nome (70–75%) ficaram intactas com "Concluir sem remover".
  - Histórico registrou `uninstall`, `forced-uninstall` e `end-process`.
- **CLI:** `hdcleaner uninstall --forced ["Nome"] [--exe X] [--folder P] [--program "Nome registrado" | --unregistered] [--end-processes] [--run-official] [--level …] [--remove-leftovers] [--confirm] [--dry-run]`.
  - Mesmos passos do diálogo: mostra alvo, programas correspondentes, processos e sobras.
  - Encerrar processos, rodar o desinstalador e remover exigem `--confirm`. Sem `--end-processes`, avisa que arquivos em uso não serão removidos.
  - Verificado: programa quebrado com processo rodando → processo encerrado, "desinstalador indisponível" informado, 7 sobras removidas com backup.
- **Programas instalados depois de a lista ser carregada:** a desinstalação forçada e a busca de sobras (todas as modalidades) releem a lista de programas na hora. Antes, um programa instalado depois do último "atualizar" não aparecia como correspondência e, pior, suas pastas não eram protegidas como "de outro programa instalado".
- A mensagem "desinstalador oficial indisponível" agora é traduzida ("Não encontrado — caminho").

### Fase 8 — Processos, inicialização e Target Mode

- **Gerenciador de processos** (página Processos):
  - Colunas: nome + descrição, PID, CPU (% da máquina entre duas amostras), memória privada, usuário e fabricante; atualiza a cada 2 s (pode pausar; para quando a janela está oculta).
  - Processos do Windows ocultos por padrão (botão para mostrar); ícone extraído do .exe sem executá-lo.
  - Painel: arquivo, versão, usuário, memória privada e conjunto de trabalho, tempo de CPU, início, threads, PID do pai, filhos; **programa instalado** dono do .exe (com Desinstalar / Mostrar em Programas); **itens de inicialização** que abrem o mesmo .exe (com Desabilitar).
  - **Encerrar processo / árvore:** confirmação; cada processo é reverificado (PID + caminho, contra reuso de PID); filhos primeiro, só os criados depois do pai; nunca encerra componentes do Windows (ficam como "mantidos"), o PID 0/4 nem o próprio app. Acesso negado → botão **Encerrar como administrador** (helper elevado, UAC único, mesmas verificações lá dentro). Histórico: `end-process` / `end-process-tree`.
- **Gerenciador de inicialização** (página Inicialização):
  - Fontes: Run/RunOnce do HKCU e do HKLM 64 e 32 bits, pastas Inicializar do usuário e de todos, tarefas agendadas com gatilho de logon/boot (fora de `\Microsoft\`) e serviços Win32 de terceiros com início automático.
  - **Desabilitar/Habilitar** pelo mesmo mecanismo do Windows (o Gerenciador de Tarefas mostra o mesmo estado): `StartupApproved\Run|Run32|StartupFolder` (12 bytes: flag + data), flag Enabled da tarefa, serviço automático ↔ manual (os que o Nexus passou para manual continuam listados para voltar). RunOnce não pode ser desabilitado.
  - **Remover** oferece "Desabilitar em vez disso" e sempre faz backup antes: `.reg` do valor Run, cópia do atalho, XML da tarefa. Componentes do Windows só podem ser desabilitados; serviços nunca são removidos.
  - Antes de qualquer ação o item é relido e o comando precisa ser o mesmo que o usuário viu (`ChangedSinceReview`).
  - HKLM, pasta de todos e serviços pedem UAC (helper elevado com novas operações tipadas `Startup` e `TerminateProcess`; o helper recusa valores do HKCU, porque o HKCU dele pode ser de outra conta).
  - Marca **arquivo não encontrado** (entradas órfãs), Windows e Admin; mostra fabricante e programa instalado; histórico `startup-enable` / `startup-disable` / `startup-remove`.
  - Verificado nesta máquina: 37 itens reais; Riot Vanguard e OneDrive apontam para arquivos que de fato não existem mais.
- **Target Mode** (item "Modo Alvo" na barra lateral e botão na página Processos):
  - O app minimiza; uma camada quase transparente cobre todos os monitores com cursor de mira; uma moldura verde destaca a janela sob o cursor e um rótulo no topo mostra o processo e o título. Clique escolhe; Esc, botão direito ou 2 minutos cancelam; o app volta sozinho.
  - Ignora janelas invisíveis, "cloaked", transparentes a cliques e as do próprio app; usa os limites visíveis (DWM). Apps da Store: o processo real é resolvido através do ApplicationFrameHost. Barra de tarefas/área de trabalho são identificadas como explorer.exe, com aviso.
  - Resultado: título, classe, PID e o mesmo painel de processo (encerrar, árvore, abrir pasta, propriedades, programa instalado + desinstalar, desabilitar na inicialização) mais **Pesquisar este arquivo no analisador** (varre a unidade se preciso e depois busca) e **Analisar espaço da pasta**.
  - Verificado com clique real: Notepad (PID certo, bloqueado para encerrar por ser do Windows), barra de tarefas (Shell_TrayWnd / explorer.exe com aviso) e cancelamento por Esc.
- **Impacto na inicialização** (botão "Ler impacto na inicialização"): lido do próprio log do Windows `Diagnostics-Performance/Operational` — evento 100 (tempo de cada boot: total, até a área de trabalho, depois dela, nº de apps) e 101/102/103 (apps, drivers e serviços que atrasaram o boot e quanto). Nada é estimado.
  - Coluna "Atraso no boot" (ordenável), média/pior/nº de boots/data no painel, "Última inicialização … · média das últimas N" no cabeçalho, e a lista de outros programas que atrasam o boot sem serem itens da lista.
  - Lançadores são seguidos: `Discord\Update.exe --processStart Discord.exe` casa com os `Discord\app-1.0.x\Discord.exe` medidos pelo Windows, somando as versões.
  - O log exige administrador: lido direto quando o app está elevado, senão pelo helper (nova operação `ReadBootPerformance`, só leitura). Nesta máquina: boots de 39–51 s; Discord +20–33 s, Steam +24,6 s, Spotify +9,4 s (7 boots).
- **Ícones da bandeja (Windows 10):** no Modo Alvo, clicar num ícone da área de notificação identifica o processo dono (lendo a barra de ícones do explorer, só leitura) e mostra a dica do ícone; o rótulo da mira também mostra o ícone sob o cursor. Verificado: Discord → Discord.exe; Rede e Volume → explorer.exe. No layout do Windows 11 (XAML) a tela diz que não é suportado. Ícones escondidos na seta ^ precisam estar visíveis na barra.
- **Processos do SYSTEM / de outras contas:** o app normal não consegue nem ler o caminho deles; "Encerrar como administrador" agora funciona assim mesmo — o helper identifica o processo por PID + nome + PID do pai (e pelo caminho quando conhecido), monta a árvore ele mesmo e mantém as mesmas proteções (nunca encerra componentes do Windows).
- **Verificado com UAC real** (aprovação automática nesta máquina; o terminal de teste **não** era administrador, então tudo passou pelo helper): desabilitar/habilitar e remover itens do HKLM Run (64 e 32 bits), atalho da pasta Inicializar de todos, tarefa criada por administrador (a tentativa normal é negada e cai no helper) e serviço (Automático ↔ Manual; remoção recusada); backups `.reg`, `.lnk` e `.xml` criados; encerrar árvore de processos do SYSTEM (conhost mantido). Depois de remover um item do HKLM, o valor dele em `StartupApproved` também é apagado, na mesma chamada elevada.
- **CLI:** `hdcleaner startup [filtro]`, `hdcleaner startup --disable|--enable|--remove "Nome" [--id ID] --confirm`, `hdcleaner startup --impact`, `hdcleaner processes [filtro] [--sort memory|cpu|name] [--top N]` e `hdcleaner kill PID [--tree] [--elevated] --confirm` (com `--json` onde lista).
- **Testes:** `startup_flow` (desabilitar/habilitar/remover item Run e atalho da pasta Inicializar com os bytes do StartupApproved, backup e recusa de revisão desatualizada), `process_tree` (filhos primeiro, caminho errado recusado antes de tocar em qualquer processo) e testes unitários de StartupApproved, enumeração e da identificação de janela.
### Fase 9 — Limpeza e limpeza de navegadores

- **Sempre em dois passos:** a análise só lista (arquivo, tamanho, data; valor do Registro com o texto). A limpeza remove **apenas os itens daquela lista** e só se continuarem iguais (mesmo tamanho e data) e dentro das pastas fixas da categoria. Nada é limpo automaticamente.
- **Proteções:** links e junções nunca são seguidos nem atravessados (teste automatizado cria uma junção apontando para fora e confere que o alvo continua lá); pastas temporárias só oferecem arquivos com **mais de 24 h** (instalações em andamento ficam intactas); arquivos em uso são pulados e contados; a pasta raiz da regra nunca é apagada, só subpastas que ficaram vazias; um item que não veio da análise é recusado.
- **Categorias (37 nesta máquina):**
  - *Sistema:* temporários do usuário e do Windows, despejos de falhas de apps e de memória do sistema, relatórios de erro (usuário e sistema), cache de miniaturas, cache de shaders (DirectX/NVIDIA/AMD) e Lixeira.
  - *Aplicativos (cache e logs, com o app fechado):* Discord, Discord PTB, Discord Canary, VS Code, Teams clássico, Spotify e Steam.
  - *Navegadores* (Chrome, Edge, Brave, Vivaldi, Opera, Opera GX e Firefox, por perfil): cache, histórico, lista de downloads, cookies, sessões e abas, dados de sites.
  - *Itens recentes:* Documentos recentes, Jump Lists, caixa Executar, endereços digitados no Explorer e arquivos recentes do Office.
- **Cookies, sessões e dados de sites são "Perigoso" e nunca vêm pré-marcados** (teste automatizado garante isso), com aviso de que desconectam dos sites. Só categorias Seguras com itens vêm marcadas.
- **Navegador/app aberto:** detectado pelo processo (Opera e Opera GX são distinguidos pela pasta), a categoria fica bloqueada e a tela diz para fechar o programa.
- **Lista de downloads do Chromium:** remove só as linhas das tabelas `downloads` do `History` (o histórico de navegação continua). O banco é lido em modo somente leitura/immutable na análise.
- **Firefox** (histórico e lista de downloads): ficam no mesmo banco dos favoritos (`places.sqlite`). São removidos linha a linha — visitas e páginas sem favorito saem, páginas favoritadas ficam — e antes de qualquer alteração o banco é copiado inteiro (cópia consistente por `VACUUM INTO`) para a pasta de backup. Teste automatizado confere que o favorito sobrevive e que a lista de downloads esvazia.
- **Lixeira:** cada item é listado com o **caminho original** (lido do arquivo de metadados `$I`), e a limpeza remove só os itens analisados — não existe "esvaziar tudo" às cegas. Teste automatizado joga um arquivo próprio na Lixeira, limpa só ele e confere que os demais itens continuam lá.
- **Listas do Registro** (Executar, endereços digitados, Office): exportadas para um `.reg` antes de apagar; só valores das chaves permitidas são tocados; a tela mostra o texto de cada item.
- **Pastas do Windows:** um usuário comum não consegue nem listar `C:\Windows\Temp`. A tela marca essas categorias como "precisa de administrador para listar" e o botão **Listar pastas do Windows (admin)** as lista pelo helper (leitura); a limpeza delas também vai pelo helper, num único UAC.
- Cada categoria mostra itens, tamanho, risco, avisos e "N arquivos com menos de 24 h mantidos"; dá para **ver a lista exata** de itens antes de limpar. Há **Simulação** e o resultado por categoria (removidos, liberados, em uso, alterados, sem permissão, falhas). Tudo vai para o histórico (`cleanup`).
- **Fechar o programa pela tela:** quando um navegador ou app está aberto, a categoria mostra o botão "Fechar <programa>". O pedido é o mesmo de clicar no X da janela (`WM_CLOSE`) — nada é forçado, então um programa com trabalho não salvo pode perguntar e continuar aberto; nesse caso a tela avisa que ele continua aberto. Depois de fechar, a análise refaz sozinha e a categoria destrava.
- **Aplicativos além da lista fixa:** além de Discord/VS Code/Teams/Spotify/Steam, o catálogo **descobre** pastas de cache em `%AppData%` e `%LocalAppData%` (`Cache`, `Code Cache`, `GPUCache`, `CachedData`, shaders…), em até dois níveis. Quando o nome da pasta casa com um programa instalado, a categoria usa o nome do programa e vem pré-marcada como Segura; quando não casa, aparece como **Revisar e não vem marcada**. Pastas vazias, pastas do Windows, dos navegadores já cobertos e **a pasta do próprio HD Cleaner** ficam de fora. Nesta máquina: 70 categorias no total (CapCut 793 MB, WhatsApp 718 MB, Riot Client 326 MB, CapoRhythia 277 MB…).
- **CLI:** `hdcleaner cleanup` (análise), `hdcleaner cleanup --items <id>`, `hdcleaner cleanup --run <ids|default> --confirm|--dry-run`.
- **Verificado:**
  - Sandbox (pastas temporárias no lugar de TEMP/LOCALAPPDATA/APPDATA, com perfil falso do Chrome e do Discord): análise, simulação e limpeza real pela tela; cache e temporários antigos removidos; cookies, favoritos, dados de site, arquivo recente (< 24 h) e cache do Discord (aberto) mantidos; downloads apagados do banco sem perder o histórico; itens recentes e Jump Lists limpos.
  - No PC real: a caixa **Executar** foi limpa (10 valores) e depois **restaurada a partir do backup .reg** gerado pelo próprio Nexus — o backup funciona.
  - Categoria de administrador pelo helper: `C:\Windows\Temp` (ver a nota no fim desta seção).
  - Análise completa em ~0,6 s.
  - Fechar programa: testado com um app de teste pela tela (fechou e a categoria destravou) e, no teste automatizado, com o Bloco de Notas — ele fecha sozinho ao ser pedido, sem ser morto; um processo sem janela não é incomodado.

### Fase 10 — Monitor de instalação

- **Três passos, só leitura:** *Iniciar monitoramento* tira um retrato do sistema (pastas de instalação, Registro, serviços, tarefas agendadas e programas instalados); o usuário roda o instalador; *Concluir e salvar* tira outro retrato e compara. O monitor nunca altera nada — o rastro é só um registro, que pode ser apagado quando quiser.
- **Retrato:** pastas `C:\Program Files`, `C:\Program Files (x86)`, `C:\ProgramData`, `%LocalAppData%`, `%AppData%` e as duas Áreas de Trabalho (até 12 níveis, com teto de arquivos); Registro em `SOFTWARE` (64 e 32 bits e HKCU), serviços, `App Paths`, `Classes`, as chaves de desinstalação e o `Installer\UserData`; valores de `Run`/`RunOnce`; lista de serviços, tarefas e programas.
- **Durante a instalação:** `ReadDirectoryChangesW` por pasta raiz (cancelado com `CancelIoEx` ao terminar) registra também **arquivos que só existem durante a instalação** — eles ficam marcados como temporários e não entram na desinstalação.
- **Atribuição conservadora:** outro programa quase sempre escreve nas mesmas pastas enquanto um instalador roda (aconteceu de verdade no teste: o Spotify se atualizou no meio). Só entra como "deste programa" o item cujo caminho carrega o nome do programa detectado (ou o nome que o usuário deu); o resto aparece separado, como **"Outras alterações no mesmo período"**, e **nunca** é proposto para remoção. Teste automatizado cobre exatamente esse caso.
- **Rastro salvo no banco** (`install_traces`, `install_trace_files`, `install_trace_registry`) com data, duração, contagens, tamanho e o programa detectado. Dá para **exportar** para JSON (levar para outra máquina), **importar** e **excluir**.
- **Usado na desinstalação:** ao desinstalar, as sobras encontradas pelas regras são somadas às do rastro (confiança 95%, motivo "Registrado pelo monitor de instalação"), sem duplicar o que as regras já acharam e passando pelas mesmas proteções. É assim que aparecem sobras que as regras não procuram — por exemplo um arquivo solto que o instalador deixou na pasta do fabricante.
- **Verificado na tela** (app de teste `hdcleaner-fake-app`): retrato em ~8 s, 62 alterações contadas durante a instalação, rastro salvo com o programa identificado, e a desinstalação pelo rastro listou e removeu as 7 sobras — inclusive a que **só o rastro conhecia** — com backup.
- **Testes:** `monitor_flow` (retrato → observação → instalação de verdade → comparação; confere pastas, arquivos, chaves e valor de inicialização criados, o programa identificado, o ruído de outro programa marcado como não relacionado, ida e volta pelo banco e as sobras geradas pelo rastro — incluindo uma que as regras sozinhas não encontram e a ausência de duplicatas).
- **Corrigido durante a verificação:** a espera pelo desinstalador adotava processos alheios (o Windows mantém o PID do pai mesmo depois que ele morre e reaproveita PIDs) e ficava presa; agora um processo só entra na árvore se tiver começado **depois** do desinstalador, e PIDs que já morreram saem da lista. Também: o assistente mostrava uma janela vazia quando o preparo falhava, e o preparo não recarregava a lista de programas quando ela estava velha.

### Fase 11 — Backups, quarentena e restauração

- **Quarentena:** ao remover sobras, arquivos e pastas de até 16 MB (no máximo 256 MB por operação) são **copiados para o backup antes** de serem apagados — links e junções nunca são copiados, e se a remoção não acontecer a cópia é descartada. Assim a remoção deixa de ser um caminho sem volta mesmo quando a Lixeira não é usada.
- **Manifesto:** toda operação destrutiva grava um `backup.json` com o tipo (desinstalação, forçada, limpeza, inicialização), o rótulo, a data e a lista do que foi salvo, com o **caminho de origem** de cada item. É isso que torna a restauração possível.
- **Restaurar do Registro:** `regops::import` lê o `.reg` que o próprio app exportou (UTF-16, `dword:`, `hex(N):`, linhas continuadas) e regrava os valores. Só escreve o que o app teria permissão de **apagar** — a mesma allowlist —, então um arquivo editado à mão não vira um caminho para escrever em qualquer lugar do Registro; diretivas de exclusão (`[-HKEY…]`) são ignoradas, porque restaurar nunca remove nada. Teste automatizado: exporta, apaga, restaura e confere valor a valor, e recusa um `.reg` apontando para `Policies`.
- **Página Backups:** lista data, tipo, rótulo, número de itens, tamanho e quantos podem voltar; **Inspecionar** mostra item a item o que está guardado, e **Restaurar** devolve os selecionados. **Nada é sobrescrito**: um item cujo caminho original existe de novo é deixado como está e aparece como "já existe". Também dá para abrir a pasta do backup e excluir o backup.
- **Backups antigos** (gravados antes do manifesto) continuam visíveis: o app lê o `manifest.json` da versão anterior para mostrar o que aquela operação removeu e restaura as exportações do Registro; os arquivos daquela época não foram copiados, então aparecem marcados como sem cópia.
- **Ponto de restauração antes de limpezas grandes:** a confirmação da Limpeza oferece criar um ponto de restauração, **já marcado** quando a seleção passa de 2 GB ou inclui categoria perigosa/de administrador. Se o Windows recusar (Proteção do Sistema desligada, ou um ponto criado há poucos minutos), a limpeza **não acontece** e o erro é mostrado.
- **Histórico:** restaurar, excluir backup e criar ponto de restauração entram no journal (`restore`, `backup-delete`, `restore-point`).
- **Segurança:** um id de backup só pode apontar para uma pasta filha direta de `backups` (`..`, caminhos absolutos e separadores são recusados — com teste).
- **Verificado na tela:** app de teste instalado, desinstalado pelo backend do app (7 sobras removidas, quarentena gravada com 645 KB), e a página Backups restaurou os 7 itens — pastas do AppData, atalho e as duas entradas do Registro — com o `settings.json` de volta no lugar certo; restaurar de novo respondeu "já existe" sem sobrescrever. O backup foi excluído pela própria tela.
- **Testes:** ida e volta da quarentena (salvar, restaurar, recusar sobrescrita, excluir), leitura de pasta antiga sem manifesto, id que tenta sair da pasta de backups, import/export do Registro, e o `uninstall_flow` agora vai até restaurar o que foi removido de um programa de verdade.

### Fase 12 — Relatório HTML, linha do tempo e CLI

- **Relatório de armazenamento em HTML** (`report.rs`): uma página **autocontida** — sem scripts e sem nada carregado de fora — com o total, o espaço em disco, a contagem de arquivos e pastas, o espaço livre do volume, a divisão por categoria, as extensões que mais ocupam, o histograma por tamanho de arquivo e as 25 maiores pastas e os 25 maiores arquivos, cada linha com barra proporcional. Nomes de arquivo são escapados (teste automatizado confere que `<`, `>` e aspas não entram crus e que não há `<script>` nem URL externa). Sai pelo menu **Exportar → Exportar relatório HTML** (respeitando a pasta em que você está) e por `hdcleaner export <caminho> --format html --output arquivo.html`. Fica no histórico como `export`.
- **Linha do tempo** (`timeline.rs`): mostra o tamanho de uma pasta ao longo dos **snapshots que já existem** — nada é varrido na hora. Uma pasta que não está em algum snapshot aparece como "não está neste snapshot", nunca como zero; snapshots ilegíveis são contados, não adivinhados. O resumo diz quanto a pasta cresceu ou diminuiu e em quantos dias (ou horas, quando é do mesmo dia). Fica na tela **Mudanças no Disco**, e funciona mesmo sem varredura aberta, porque só lê snapshots. Na CLI: `hdcleaner timeline <caminho> [--root C:\] [--limit N]` (com `--json`).
- **`hdcleaner cleanup --analyze`**: lista sem limpar (que já era o padrão sem `--run`), e recusa a combinação `--analyze --run` em vez de fazer algo que o usuário não pediu.
- **Verificado no PC real:** relatório gerado pela CLI e pela tela (11,5 KB, números batendo com a varredura: 441 KB, 50 arquivos, 10 pastas) e linha do tempo montada a partir de 3 snapshots reais de `C:\` (30,0 GB, 72.348 → 72.349 arquivos) em ~0,5 s, pela tela e pela CLI.
- **Testes:** página autocontida e com escape correto; linha do tempo lendo vários snapshots (ordem por data, pasta ausente, arquivo ilegível) e o mapeamento de caminho para volume.

### Recursos das fases 11 e 12 adiantados

- **Exclusão segura:**
  - **Plano:** o backend resolve o caminho final passando por junções, classifica o risco e grava uma *fingerprint* (serial do volume + id do arquivo + tamanho + data de modificação).
  - **Revisão:** o usuário vê exatamente o que acontecerá.
  - **Execução reverificada:** a *fingerprint* e a proteção são checadas de novo (defesa contra TOCTOU).
  - Modos: Lixeira (`IFileOperation`) ou exclusão permanente.
  - Existe **dry run**.
  - Um link simbólico ou junção é removido **sem apagar o destino**.
  - A Lixeira só é oferecida em unidades fixas.
- **Protection Engine** com as classificações SAFE, REVIEW, DANGEROUS e BLOCKED:
  - **Bloqueados:** pasta do Windows (exceto `Windows\Temp`), boot, EFI, pagefile, metadados NTFS, raízes de unidade, pastas de topo e perfis inteiros, WindowsApps e o hive do usuário.
  - Itens DANGEROUS exigem uma confirmação extra.
- **Histórico e journal:**
  - Registra varreduras, exportações, exclusões, simulações, renomeações e transferências, e pode ser exportado em JSON.
  - Operações interrompidas geram uma janela com as opções Retomar, Inspecionar e Descartar. Retomar gera **um novo plano revalidado**.
- **Snapshots `.hdcs`:**
  - Formato binário com LZ4, validado como entrada não confiável.
  - Salvos automaticamente a cada varredura (mantendo os 12 mais recentes por local).
  - Podem ser abertos e salvos pelo usuário.
- **Mudanças no Disco:** compara a varredura atual com um snapshot anterior e mostra a diferença total e os arquivos e pastas que mais cresceram ou diminuíram.
- **Exportação CSV/JSON:**
  - Funciona para a varredura inteira ou para os resultados de uma busca.
  - O CSV tem BOM para abrir corretamente no Excel e proteção contra injeção de fórmula.
- **Gerenciamento de arquivos:**
  - abrir e abrir pasta (seleciona o item no Explorer);
  - copiar caminho e propriedades;
  - renomear (nunca sobrescreve) e copiar/mover (pelo diálogo do Windows);
  - Lixeira e exclusão permanente;
  - terminal e PowerShell na pasta.
- **CLI `hdcleaner`:**
  - comandos `drives`, `scan` (com `--elevate`, `--snapshot`), `largest`, `search`, `duplicates`, `export`, `diff`, `delete`, `programs`, `uninstall`, `startup`, `processes`, `kill` e `cleanup`;
  - `delete` exige `--confirm` e aceita `--dry-run`;
  - `cleanup` analisa e limpa as categorias escolhidas.
- **Atalhos de teclado:**
  - Ctrl+F busca, F5 rescan, Ctrl+L foca o campo de caminho, Esc cancela.
  - Del envia para a Lixeira, Shift+Del exclui permanentemente, F2 renomeia, Ctrl+C copia o caminho.
  - Setas, Enter e Home/End nas tabelas.
- **Dashboard:**
  - unidades com barra de uso;
  - "O que está ocupando meu armazenamento?" por categoria (cada categoria abre a busca filtrada);
  - resumo e observações conservadoras, sem mandar apagar nada;
  - ações rápidas.

### Bugs encontrados em testes reais e já corrigidos

1. Os eventos de varredura chegavam com os campos em *snake_case*, e o resultado não aparecia na tela.
2. Depois de uma exclusão, os totais do cabeçalho e o número de linhas da árvore ficavam desatualizados.
3. O `$BadClus` era contado como 473 GB na varredura MFT.
4. "7-Zip 23.01 (x64)" não casava com a pasta "7-Zip".
5. Caminhos com `/` no Registro (caso do Riot) eram rejeitados.
6. O Discord ficava sem ícone porque não tem DisplayIcon e o `Update.exe` dele não tem ícone.
7. Depois de um lote, o botão "Desinstalar selecionados (2)" continuava aparecendo com programas que já não existiam.
8. A busca de sobras usava a lista de programas em cache: um programa instalado depois do último carregamento não tinha as pastas protegidas.
9. Na CLI, a coluna de nível da lista de sobras ficava desalinhada.
10. Serviços "manuais" por natureza (Elevation Services, updaters) apareciam como "desabilitados" na inicialização; "habilitá-los" os deixaria automáticos. Agora só aparecem serviços automáticos e os que o próprio Nexus desativou.
11. "Arquivo não encontrado" podia ser falso em pastas com acesso negado; agora só marca quando o Windows diz que o arquivo não existe.
12. "Encerrar árvore" contava o conhost.exe (componente do Windows, protegido) como falha; agora ele aparece como "mantido".
13. **Travamento (deadlock)** em "Calcular tamanhos reais": ao encontrar um programa sem tamanho em cache, o comando tentava travar de novo o mesmo cadeado que ainda segurava (temporário do `match` em Rust). O mesmo padrão foi evitado no leitor de impacto.
14. O Steam aparecia como "Overwatch®": jogos da Steam registram `steam.exe` como desinstalador e "reivindicam" a pasta da Steam. Em empate, agora vence o programa cujo nome bate com o arquivo/pasta.
15. Processos do SYSTEM ou de outras contas não podiam ser encerrados nem como administrador (o app nem lia o caminho deles).
16. A simulação de uma categoria do Windows falhava com "requer administrador", mesmo sem remover nada.
17. Na tela de Limpeza, a lista de itens duplicava as linhas e o resumo do que foi removido sumia logo depois da limpeza.

### Nota sobre um teste que foi longe demais

Durante o teste da categoria "Temporários do Windows", a limpeza real foi executada no PC do usuário e removeu **8.763 arquivos (892 MB) de `C:\Windows\Temp`** com mais de 24 h — o mesmo que a Limpeza de Disco do Windows remove, mas sem o usuário ter pedido. A remoção é permanente. A intenção era limpar apenas dois arquivos de teste; a categoria cobre a pasta inteira. Todos os outros testes em dados reais foram feitos em **simulação** ou com **restauração pelo backup** (caixa Executar).

---

## 3. O que ainda falta

Legenda: 🔴 não iniciado · 🟡 parcial


### Fase 12 — CLI, exportação e snapshots 🟡

- Já existe: tudo, exceto um item opcional.
- Falta:
  - exportar e importar a **MFT bruta** para análise offline — marcado como opcional na especificação e **não implementado**; a leitura da MFT continua sendo feita ao vivo, e o snapshot `.hdcs` já serve para levar uma varredura para outra máquina.

### Fase 13 — Integração total analisador ↔ desinstalador 🟡 (próxima)

- Já existe: tamanho real, App Storage Map, identificar programa e aplicativo relacionado.
- Falta:
  - **App Analyzer completo** (seção 42): processos atuais, inicialização relacionada, chaves do Registro conhecidas, "Limpar cache" (só itens classificados com segurança como cache) e "Desabilitar inicialização";
  - **Uninstall Impact** (seção 66-C): programa, cache e dados do usuário antes de desinstalar, com o que será removido e o que será mantido;
  - **Smart Storage** completo (seção 41):
    - Jogos → listar os jogos (bibliotecas Steam, Epic, Riot…);
    - Caches → mostrar qual aplicativo gerou cada cache;
    - Aplicativos → relacionar arquivos aos programas;
  - **Correlation engine** (seção 69) com mais sinais: atalhos, caminhos de processos, traces, fabricante e pontuação numérica.

### Fase 14 — Desempenho, segurança e acabamento 🔴

- Perfilar e reduzir cópias de strings; medir a memória em varreduras de 5 milhões ou mais de arquivos.
- **Monitoramento incremental** do sistema de arquivos após a varredura (seção 43), com USN Journal ou `ReadDirectoryChangesW`.
- **Assinatura Authenticode** e **proprietário** no painel de detalhes (seções 67 e 68). Hoje aparecem marcados como "Ainda não implementado".
- Mostrar **File ID** e contagem de hard links também na varredura padrão (hoje só a MFT fornece a contagem).
- Colunas "Quantidade de duplicados" e "Espaço duplicado" na file view (seção 9).
- Agrupamento por **proprietário** em Arquivos Grandes.
- **Fila visual de operações** longas de arquivo, com progresso (seção 13); hoje a cópia e a movimentação usam o diálogo do Windows.
- Atalhos que faltam: Ctrl+H e Ctrl+X (recortar) (seção 54).
- Acessibilidade: revisão com leitor de tela e testes nos níveis de DPI 100%, 125%, 150% e 200% (seção 53).
- **Menu de contexto do Explorer**, "Analisar com HD Cleaner", com opção de remover (seção 55).
- Configurações "Iniciar com o Windows" e "Verificar atualizações" (hoje desabilitadas e marcadas).
- **Updater assinado**, com validação de assinatura e integridade (seção 63).
- **Secure Delete** com métodos de sobrescrita configuráveis e aviso claro sobre SSDs (seção 35).
- **Limpeza do espaço livre** com confirmação explícita e estimativa do volume a gravar (seção 36).
- Suporte a dispositivos **MTP/PTP** (seção 5), se for viável.
- MFT com `$ATTRIBUTE_LIST` no registro 0: hoje cai para a varredura padrão; o ideal é suportar leitura completa.
- **Windows Apps** (seção 23): remover para o usuário atual ou para todos, redefinir, reparar e ver o pacote e as dependências, com alertas para componentes críticos. A listagem já existe na página Programas.
- **Extensões de navegadores** (seção 24): Chrome, Edge, Firefox, Brave, Opera e Vivaldi, com nome, ID, versão, pasta, tamanho, estado e remoção segura.
- Gerar os instaladores **NSIS/MSI** (`npx tauri build`) e testar em ARM64.
- Testes adicionais pedidos (seção 59): arquivos sparse, arquivos enormes e testes de integração da interface.
- Dar um nome definitivo ao produto (hoje é provisório).
- Resolver os avisos de estilo do `clippy` (nenhum é de correção).

---

## 4. Riscos e observações

- A varredura MFT foi validada apenas no C:\ desta máquina (SSD, Windows 10 19045). Ainda precisa ser testada no D:\ (HDD), em volumes com MFT muito fragmentada e em Windows 11.
- A integração entre programas e pastas é feita por **pasta registrada**, **pasta do desinstalador** e **nome da pasta**. Correspondências só por nome são mostradas como "possivelmente relacionadas" e **nunca** devem servir de base para exclusão automática.
- Operações destrutivas continuam exigindo plano, revisão e reverificação. Nenhuma decisão de exclusão é tomada por heurística ou IA.
