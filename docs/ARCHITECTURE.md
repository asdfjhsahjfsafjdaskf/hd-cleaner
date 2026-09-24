# HD Cleaner — Arquitetura e Estado do Projeto

> Nome provisório. Todos os nomes de produto ficam em
> `crates/hdcleaner-core/src/branding.rs`, `src/config/branding.ts` e
> `src-tauri/tauri.conf.json` (`productName`, `title`).

## 1. Visão geral

```
UI (React/TS, src/)                      ← apenas apresentação; nunca toca o disco
  │  invoke()/Channel (IPC Tauri, JSON; treemap em binário)
Camada de aplicação (src-tauri/src/commands)
  │  valida parâmetros, mantém estado (scans carregados, views, resultados)
Serviços de sistema (crates/hdcleaner-core)   ← toda a lógica, compartilhada com a CLI
  │  scan, mft, search, treemap, duplicates, protection, fsops, db, elevation…
Windows APIs (windows-sys / windows-rs), sistema de arquivos, Registro
```

| Caminho | Conteúdo |
|---|---|
| `crates/hdcleaner-core` | Biblioteca com todos os serviços (testável sem UI) |
| `crates/hdcleaner-cli` | Binário `hdcleaner` (CLI) |
| `src-tauri` | App desktop Tauri 2: comandos, estado, DTOs |
| `src` | Frontend React + TypeScript + Vite |

### Módulos do core

| Módulo | Responsabilidade |
|---|---|
| `scan::model` | Árvore compacta: `Vec<Node>` (76 B/nó), nomes em pool UTF-8 único, filhos em CSR ordenados por tamanho alocado, extensões internadas (u16) |
| `scan::builder` | Construção incremental + agregação bottom-up (hard links contados uma vez) |
| `scan::standard` | `WindowsApiScanner`: `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)` em paralelo (rayon), fallback `FindFirstFileExW(LARGE_FETCH)`; caminhos longos (`\\?\`), junções não seguidas por padrão, placeholders de nuvem detectados |
| `scan::mft` | `NtfsFastScanner`: leitura direta da MFT (`FSCTL_GET_NTFS_VOLUME_DATA`, runlist do `$MFT`, fixups, `$STANDARD_INFORMATION`/`$FILE_NAME`/`$DATA`, registros de extensão, sparse/comprimido) com leitor em thread separada e parsing paralelo |
| `scan::snapshot` | Formato binário `.hdcs` (LZ4, validação completa na leitura — tratado como entrada não confiável) |
| `search` | Linguagem de busca (`size:`, `modified:`, `ext:`, `path:`, `type:`, curingas, negação) |
| `treemap` | Layout *squarified* em Rust; saída binária de 24 B por retângulo |
| `duplicates` | tamanho → hash parcial BLAKE3 (início+fim) → hash completo só dos candidatos |
| `protection` | Protection Engine: SAFE / REVIEW / DANGEROUS / BLOCKED |
| `fsops` | Plano de exclusão com *fingerprint* → reverificação → Lixeira (`IFileOperation`) ou permanente; renomear sem sobrescrever; abrir, Explorer, propriedades, terminal |
| `elevation` | Helper elevado de superfície mínima via UAC + named pipe (`scan-ntfs`, `apply-ops` com operações tipadas e revalidadas) |
| `uninstall`, `leftovers`, `regops`, `shortcuts`, `sysitems` | Desinstalador oficial com espera da árvore de processos; motor de sobras; backup .reg + allowlist de exclusão no Registro; alvos de .lnk; Run/serviços/tarefas |
| `correlate` | De quem é esta pasta: sinais nomeados (local registrado, rastro, processo, atalho, fabricante, nome) com nota 0-100 |
| `appanalysis` | Processos, inicialização, chaves e caches de um programa; base do painel e do Uninstall Impact |
| `smartstorage` | Jogos lidos dos arquivos dos launchers (Steam/Epic/Riot) e caches com o dono identificado |
| `fileinfo` | Dono do arquivo e assinatura Authenticode (embutida ou por catálogo do Windows) |
| `extensions` | Extensões de navegador lidas dos arquivos do próprio navegador |
| `clipboard` | Arquivos na área de transferência do Windows (recortar/copiar para o Explorer) |
| `report` | Relatório de armazenamento em HTML autocontido (sem scripts nem recursos externos) |
| `timeline` | Tamanho de uma pasta ao longo dos snapshots salvos; pasta ausente nunca vira zero |
| `backups` | O que foi salvo antes de remover: manifesto, quarentena, restauração (nunca sobrescreve) e exclusão |
| `monitor` | Retrato do sistema antes/depois de uma instalação + `ReadDirectoryChangesW` durante ela; o rastro marca o que é do programa e o que é ruído de outro |
| `db` | SQLite: settings, operações (journal), scans/snapshots, rastros de instalação |
| `diff`, `export`, `stats`, `tools`, `registry`, `disk`, `system` | Comparação de snapshots, CSV/JSON, agregações, atalhos do Windows, leitura do Registro, unidades, token/SO |

## 2. Segurança

* **Nada destrutivo no frontend.** A UI só envia *ids de nós* ou caminhos; o backend
  normaliza (`normalize_root` rejeita caminhos relativos, `..`, NUL), classifica e executa.
* **Plano → confirmação → execução.** `plan_delete` resolve o caminho final
  (`GetFinalPathNameByHandleW`, pega junções nos pais), classifica e grava um
  *fingerprint* (serial do volume + file id + tamanho + mtime). `execute_delete`
  reabre cada item, compara o fingerprint (**defesa TOCTOU**) e reavalia a proteção
  antes de agir. Itens alterados falham com `changedSinceReview`.
* **Protection Engine** bloqueia: diretório do Windows (exceto `Windows\Temp`),
  raízes de volume/compartilhamento, boot/EFI/Recovery/pagefile/hiberfil, metadados
  NTFS, contêineres de topo (Program Files, ProgramData, Users, perfis, AppData…),
  WindowsApps e hive do usuário. Program Files/ProgramData ficam como *Dangerous*
  (exigem confirmação explícita adicional).
* **Junções/symlinks:** excluir um link remove só o link (`remove_dir`);
  `remove_dir_all` do std não segue reparse points.
* **Lixeira:** só é oferecida em unidades fixas; em outras o shell excluiria
  permanentemente sem aviso — nesse caso o item é recusado com explicação.
* **Menor privilégio:** o app roda sem elevação. O MFT scan usa o mesmo executável
  relançado via UAC em modo helper, que aceita **apenas** `scan-ntfs <letra> <canal>`
  (validação estrita; nenhum caminho vem do chamador). O resultado volta por named pipe
  criado pelo processo não elevado com `FILE_FLAG_FIRST_PIPE_INSTANCE` e
  `PIPE_REJECT_REMOTE_CLIENTS`. Não existe API genérica "executar como admin".
* **Sem concatenação de comandos:** terminais via `std::process::Command` com
  argumentos estruturados; ferramentas do Windows via lista branca fixa.
* **CSV:** campos iniciados por `= + - @` são neutralizados (injeção de fórmula).
* **Crash safety:** toda exclusão/transferência é registrada como `running` antes de
  começar; na inicialização, operações `running` aparecem como interrompidas
  (Retomar = novo plano revalidado / Inspecionar / Descartar). Snapshots e exportações
  são gravados em arquivo temporário e renomeados atomicamente.
* **Privacidade:** nenhuma telemetria, nenhuma chamada de rede.

## 3. Desempenho (medido nesta máquina, Windows 10, SSD)

| Operação | Resultado |
|---|---|
| Scan padrão (sem admin) de `C:\` | ~1,18 M entradas em 6,5–11,7 s |
| Busca `size:>1GB type:file` em ~1,2 M nós | 15 ms |
| Duplicados (≥ 20 MB) no perfil do usuário | 33 grupos, 2,82 GB desperdiçados, 5 GB lidos em ~6 s |
| Frontend | tabelas virtualizadas; páginas de 200 linhas sob demanda; treemap em Canvas com índice espacial |

## 4. Estado das funcionalidades

Legenda: ✅ implementado e testado · 🟡 parcial · ⛔ não implementado (marcado assim na UI)

| Área (seção do escopo) | Estado | Observações |
|---|---|---|
| Dashboard (4) | ✅ | Unidades, SSD/HDD, categorias, observações conservadoras, contagem de programas; "inicialização" marcada como não implementada |
| Scanner padrão (5) | ✅ | NTFS, FAT/exFAT, rede, pastas; progresso, velocidade, cancelamento |
| Scanner MFT (5) | ✅ | Validado no C:\ real via helper elevado (UAC): 1,26 M entradas em 3,5 s; total 345 GB = espaço usado segundo o Windows (345,5 GB). Alocado calculado pelos clusters reais do runlist (corrige `$BadClus` e streams sparse sem flag). MFT com `$ATTRIBUTE_LIST` no registro 0 → fallback explícito |
| Dispositivos MTP/PTP (5) | ⛔ | |
| Tamanho lógico × alocado, sparse, comprimido, hard links, junções, OneDrive (6) | ✅ | |
| Tree view (7) | ✅ | Ordenação, expandir/recolher tudo, estado preservado no rescan (F5) |
| Treemap (8) | ✅ | Cores por categoria, tooltip, zoom por duplo clique, breadcrumb, menu de contexto, destaque |
| File view (9) | 🟡 | Colunas de duplicados na tabela ainda não |
| Pesquisa avançada + filtros rápidos (10) | ✅ | |
| Arquivos grandes (11) | 🟡 | Top N, agrupamento, histograma; agrupamento por proprietário não |
| Duplicados (12) | ✅ | Seleção inteligente só pré-seleciona; impede excluir todas as cópias de um grupo |
| Gerenciamento de arquivos (13) | 🟡 | Abrir, renomear, copiar/mover (diálogo nativo do shell), Lixeira, permanente, copiar caminho, propriedades, terminal/PowerShell. Fila visual de operações longas ainda não |
| Exportação CSV/JSON, snapshots, comparação (14) | ✅ | Export/import da MFT bruta não |
| CLI (15) | ✅ | `drives scan largest search duplicates export (csv/json/html) timeline games diff delete programs uninstall startup processes kill cleanup` |
| Programas instalados (16) | ✅ | HKLM 64/32 + HKCU, MSI, AppX/MSIX/Store (WinRT, sem admin); ícones (DisplayIcon → desinstalador → .ico/.exe da pasta); oculta componentes de sistema/updates/frameworks por padrão; busca, filtros, ordenação; CLI `hdcleaner programs [--sizes]` |
| Tamanho real (17), App Storage Map (40/66-B), identificar programa (40) | ✅ | Instalação/dados/cache/logs; locais Confirmado / Provável / Possivelmente relacionado com motivo; usa varredura carregada quando existe; "Ver no mapa do disco" destaca as pastas no treemap; "Identificar programa instalado" no menu e no painel de detalhes |
| Desinstalação normal + busca de sobras (18–19) | ✅ | Assistente completo, 3 níveis com confiança/motivo, backup .reg, Lixeira, dry run, UAC único via helper `apply-ops`, CLI `hdcleaner uninstall`; teste de ponta a ponta com programa falso |
| Forçada, lote, rápida (20–22) | ✅ | Forçada por nome/.exe/pasta com correspondência a programas registrados e encerramento revalidado de processos; lote sequencial com UAC único; rápida remove só o inequívoco (Seguro, ≥ 90%, não compartilhado). CLI `hdcleaner uninstall --forced` |
| Windows Apps (23), extensões de navegador (24) | ✅ | `appx.rs` (listar, reparar por re-registro, remover para o usuário ou para todos; componentes do Windows recusados no app e no helper) e `extensions.rs` (manifest/Preferences do Chromium e extensions.json do Firefox; remoção só com o navegador fechado) |
| Target Mode (25), gerenciador de processos (26), inicialização (27) | ✅ | `target.rs` (camada de mira + moldura, UWP via ApplicationFrameHost), `processes.rs` (amostragem de CPU, dono, árvore reverificada), `startup.rs` (StartupApproved/tarefa/serviço; remover com backup). `bootperf.rs` (impacto medido pelo Windows, log Diagnostics-Performance via helper); ícones da bandeja no Windows 10 (Windows 11: não suportado) |
| Monitor de instalação, traces (28–29) | ✅ | `monitor.rs`: retrato antes/depois (pastas até 12 níveis, Registro nas áreas relevantes, serviços, tarefas, programas) + `ReadDirectoryChangesW` durante a instalação; comparação gera o rastro, que separa o que é do programa do que outro programa escreveu no mesmo período. Rastros no banco (`install_traces`, `install_trace_files`, `install_trace_registry`), com exportar/importar/excluir; `leftovers::from_trace` alimenta a desinstalação (só itens relacionados, sem duplicar as regras) |
| Backups/quarentena/ponto de restauração (30) | ✅ | `backups.rs`: manifesto por operação, quarentena de arquivos pequenos antes da remoção, listagem, restauração que nunca sobrescreve e exclusão; `regops::import` devolve valores do Registro sob a mesma allowlist da exclusão. Ponto de restauração oferecido antes de limpezas grandes; restaurações vão para o histórico |
| Cleaner, browser cleaner, itens recentes (31–33) | ✅ | `cleaner.rs`: catálogo de categorias, análise que só lista, limpeza restrita aos itens analisados e inalterados, junções nunca seguidas, temporários só com mais de 24 h, app/navegador aberto bloqueia, backup .reg das listas do Registro, categorias do Windows analisadas e limpas pelo helper. Firefox: histórico e downloads removidos linha a linha do places.sqlite (favoritos mantidos, banco copiado antes); Lixeira item a item com o caminho original; caches de aplicativos descobertos automaticamente; botão para fechar o programa aberto (WM_CLOSE, nunca forçado) |
| Ferramentas do Windows (34) | ✅ | |
| Secure delete (35) | ✅ | `DeleteMode::Secure { passes }`: 1-7 sobrescritas antes de apagar, links nunca seguidos, aviso explícito de SSD na tela e na CLI |
| Wipe de espaço livre (36) | ⛔ | |
| Protection Engine, classificação de risco, dry run (37–39) | ✅ | |
| App analyzer completo (42) | ✅ | Painel em Programas: processos rodando de dentro das pastas do programa, inicialização relacionada, chaves do Registro, caches com ação de limpar e desabilitar inicialização |
| Smart storage (41) | 🟡 | Categorias levam à busca filtrada; jogos lidos dos launchers (Steam/Epic/Riot) no Painel e na CLI; caches ligados ao programa que os criou. Falta relacionar arquivo a arquivo na file view |
| Monitoramento incremental (43) | ⛔ | |
| Histórico (50), configurações (51), i18n pt-BR/en (52) | ✅ | |
| Acessibilidade/atalhos (53–54) | 🟡 | Teclado nas tabelas, menus e diálogos; Ctrl+F, F5, Del, Shift+Del, F2, Ctrl+C, Ctrl+L, Ctrl+X (recorte para o Explorer), Ctrl+H (histórico), Esc; escala de fonte. Falta a revisão com leitor de tela e os testes de DPI |
| Menu de contexto do Explorer (55), relatório HTML (56), updater (63) | ⛔ | |
| Assinaturas Authenticode / proprietário no painel de detalhes (67–68) | ⛔ | Marcado na UI |
| Correlação com apps instalados (69) | 🟡 | Por pasta registrada, pasta do desinstalador e identidade de pacote; demais sinais (atalhos, processos, traces) nas próximas fases |

## 5. Como compilar e executar

Pré-requisitos: Rust (MSVC), Node 20+, Visual Studio Build Tools, WebView2.

```powershell
npm install
npx tauri dev              # app em modo desenvolvimento
cargo test --workspace     # testes (usam apenas pastas temporárias)
cargo build --release -p hdcleaner-cli   # CLI: target\release\hdcleaner.exe
npx tauri build            # instaladores NSIS/MSI
```

Dados locais: `%LOCALAPPDATA%\HDCleaner` (`hdcleaner.db`, `logs\`, `snapshots\`).

## 6. Próximos passos (roteiro)

1. Fase 14: desempenho, Windows Apps, extensões de navegador, instaladores e acabamento.
2. Monitoramento incremental (`ReadDirectoryChangesW`/USN Journal) e colunas de duplicados na file view.
