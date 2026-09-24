# Auditoria do HD Cleaner 0.1.1

Esta matriz compara o código com os 76 tópicos do escopo original. **Atendido** exige evidência de implementação e teste pertinente; **parcial** indica implementação incompleta ou validação insuficiente; **ausente** significa que o recurso não foi localizado; **não validado** significa que depende de ambiente, escala ou inspeção manual indisponível nesta rodada. A presença de um módulo não prova o comportamento da interface. Nenhuma operação destrutiva foi testada em dados reais do usuário.

| # | Requisito | Estado | Evidência e limite |
|---:|---|---|---|
| 1 | Objetivo all-in-one | Parcial | `src/pages`, `crates/hdcleaner-core/src`; integração existe, completude não comprovada |
| 2 | Stack Rust/Tauri/React | Atendido | `Cargo.toml`, `src-tauri/Cargo.toml`, `package.json`; builds passaram |
| 3 | Segurança, desempenho e Windows 10/11 | Não validado | `protection.rs`, `scan`; sem teste comparativo em Windows 11/ARM64 |
| 4 | Dashboard | Parcial | `src/pages/Dashboard.tsx`; sem teste de UI nesta rodada |
| 5 | Scanner rápido e fallback | Parcial | `scan/mft`, `scan/standard.rs`; testes sintéticos e de pasta temporária, MTP/PTP ausente |
| 6 | Tamanhos lógico e alocado | Parcial | `scan/model.rs`, `scan/mft/parse.rs`; casos reais de OneDrive/compressão não validados |
| 7 | Tree view | Parcial | `src/pages/DiskAnalyzer.tsx`, `VirtualTable.tsx`; estado no rescan sem teste de UI |
| 8 | Treemap | Parcial | `treemap.rs`, `src/components/Treemap.tsx`; layout testado, interação não validada |
| 9 | File view | Parcial | `VirtualTable.tsx`; colunas de duplicados ainda ausentes |
| 10 | Busca avançada | Parcial | `search.rs`; testes de parsing passaram, interação não validada |
| 11 | Arquivos grandes | Parcial | `src/pages/LargeFiles.tsx`; agrupamento por proprietário ausente |
| 12 | Duplicados | Parcial | `duplicates.rs`, `src/pages/Duplicates.tsx`; pipeline testado, seleção na UI não validada |
| 13 | Gerenciamento de arquivos | Parcial | `fsops.rs`, `fileActions.tsx`; fila visual de cópia/movimentação ausente |
| 14 | Exportação/importação e snapshots | Parcial | `export.rs`, `scan/snapshot.rs`, `diff.rs`; MFT bruta opcional ausente |
| 15 | CLI | Parcial | `crates/hdcleaner-cli`; build passou, comandos destrutivos não executados |
| 16 | Programas instalados | Parcial | `programs.rs`, `appx.rs`, `src/pages/Programs.tsx`; inventário real não comparado ao Windows |
| 17 | Tamanho real dos programas | Parcial | `appsize.rs`, `appanalysis.rs`; estimativas e associação requerem amostragem manual |
| 18 | Desinstalação normal | Parcial | `uninstall.rs`, `UninstallWizard.tsx`; teste com instalador fictício não executado nesta rodada |
| 19 | Sobras | Parcial | `leftovers.rs`; allowlist e associação requerem validação adicional com programas reais |
| 20 | Desinstalação forçada | Parcial | `forced.rs`, `ForcedUninstallDialog.tsx`; teste de sistema não executado |
| 21 | Desinstalação em lote | Parcial | `BatchUninstallDialog.tsx`; fluxo de instaladores não executado |
| 22 | Desinstalação rápida | Parcial | `BatchUninstallDialog.tsx`, `leftovers.rs`; comportamento completo não validado |
| 23 | Windows Apps | Parcial | `appx.rs`, `src/pages/WindowsApps.tsx`; reset do app não implementado |
| 24 | Extensões de navegador | Parcial | `extensions.rs`, `src/pages/Extensions.tsx`; parsers testados, perfis reais não removidos |
| 25 | Target Mode | Parcial | `target.rs`, `TargetDialog.tsx`; dependente de versão do Windows |
| 26 | Processos | Parcial | `processes.rs`, `src/pages/Processes.tsx`; encerramento testado só com processo criado para teste |
| 27 | Inicialização | Parcial | `startup.rs`, `src/pages/Startup.tsx`; alterações reais não executadas nesta rodada |
| 28 | Monitor de instalação | Parcial | `monitor.rs`, `src/pages/Monitor.tsx`; watcher testado em pasta temporária |
| 29 | Banco de traces | Parcial | `db.rs`, `monitor.rs`; persistência unitária testada |
| 30 | Backups e restauração | Parcial | `backups.rs`; quarentena/restauração em temp e rejeição de manifesto inválido testadas |
| 31 | Cleaner | Parcial | `cleaner.rs`, `src/pages/Cleaner.tsx`; fluxo em sandbox passou |
| 32 | Limpeza de navegadores | Parcial | `cleaner.rs`; fluxo com perfil fictício passou, versões reais não validadas |
| 33 | Itens recentes/Office | Parcial | `cleaner.rs`, `sysitems.rs`; cobertura por versão do Office não validada |
| 34 | Ferramentas do Windows | Parcial | `tools.rs`, `src/pages/Tools.tsx`; allowlist testada, abertura da UI não validada |
| 35 | Exclusão segura | Parcial | `fsops.rs`; teste de arquivo temporário passou, limitações de SSD permanecem |
| 36 | Limpeza de espaço livre | Parcial | `wipe.rs`, `commands/files.rs`; teste que grava no volume do sistema foi excluído |
| 37 | Proteção contra erros | Parcial | `protection.rs`, `fsops.rs`; caminhos críticos e links testados |
| 38 | Classificação de risco | Parcial | `protection.rs`; regras unitárias passaram, diálogo não validado |
| 39 | Dry run | Parcial | `fsops.rs`, `cleaner.rs`; fluxo de limpeza em sandbox verificou preservação |
| 40 | Integração disco/programas | Parcial | `correlate.rs`, `appanalysis.rs`, `Programs.tsx`; UI não validada |
| 41 | Smart Storage | Parcial | `smartstorage.rs`; ainda falta associação arquivo a arquivo |
| 42 | App Analyzer | Parcial | `appanalysis.rs`, `Programs.tsx`; dados reais não auditados |
| 43 | Monitoramento incremental após scan | Ausente | Não localizado fluxo que atualize a árvore após mudanças no disco |
| 44 | Rescan preservando estado | Parcial | `stores/analyzer.ts`; sem teste de UI |
| 45 | UI/UX | Não validado | Páginas implementadas, sem revisão visual completa nesta rodada |
| 46 | Performance | Não validado | Sem profiling recente em 5 milhões de arquivos nem medições de RAM |
| 47 | Administração e UAC | Parcial | `elevation.rs`; protocolo de helper inspecionado, UAC não exercitado |
| 48 | Segurança de caminhos/comandos | Parcial | `protection.rs`, `fsops.rs`, `backups.rs`; não equivale a auditoria independente |
| 49 | SQLite e retenção | Parcial | `db.rs`; migração testada, escala de milhões não validada |
| 50 | Histórico | Parcial | `db.rs`, `src/pages/History.tsx`; persistência testada |
| 51 | Configurações | Parcial | `src/pages/Settings.tsx`; updater e algumas opções ainda indisponíveis |
| 52 | Português e inglês | Parcial | `src/i18n/en.ts`, `ptBR.ts`; cobertura de todas as mensagens não medida |
| 53 | Acessibilidade/DPI | Não validado | Sem leitor de tela nem matriz 100/125/150/200% |
| 54 | Atalhos | Parcial | `DiskAnalyzer.tsx`, `App.tsx`; sem teste de teclado completo |
| 55 | Menu do Explorer | Parcial | `shellmenu.rs`, `Settings.tsx`, `App.tsx`; fluxo ligado, teste real do Registro ignorado |
| 56 | Relatórios | Parcial | `report.rs`, `export.rs`; formato HTML testado em unidade, UI não validada |
| 57 | Recomendações conservadoras | Parcial | `Dashboard.tsx`; textos e dados não revisados em execução |
| 58 | IA não decide exclusões | Atendido | `protection.rs`, `fsops.rs`; regras determinísticas e confirmação, sem integração de IA |
| 59 | Testes gerais | Parcial | 120 unitários seguros e 1 integração de limpeza passaram; cenários restantes não executados |
| 60 | Testes de desinstalador | Parcial | `crates/test-fixtures`; fixtures existem, testes que criam entradas reais não executados |
| 61 | Tratamento de erros | Parcial | `error.rs`, `ErrorView.tsx`; nem todos os erros foram induzidos |
| 62 | Segurança contra crash | Parcial | `db.rs`, `InterruptedDialog`; retomada real não validada |
| 63 | Updater assinado | Ausente | Controle em Configurações está desativado; sem servidor/chaves de assinatura |
| 64 | Telemetria opt-in | Parcial | Nenhum SDK de telemetria localizado; tráfego não monitorado nesta rodada |
| 65 | Privacidade/localidade | Não validado | Sem inspeção de tráfego em execução |
| 66 | Diferenciais de armazenamento | Parcial | `appsize.rs`, `diff.rs`, `timeline.rs`, `correlate.rs`; UX e precisão não validadas |
| 67 | Detalhes do arquivo | Parcial | `fileinfo.rs`, `DetailsPanel.tsx`; nem todos os campos confirmados em runtime |
| 68 | Assinaturas digitais | Parcial | `fileinfo.rs`; teste com arquivo do Windows passou |
| 69 | Correlação de aplicativo | Parcial | `correlate.rs`; falsos positivos em programas reais não medidos |
| 70 | Arquitetura do scanner | Parcial | `scan/mod.rs`, `scan/standard.rs`, `scan/mft`; MTP/PTP ausente |
| 71 | Arquitetura do desinstalador | Parcial | `uninstall.rs`, `forced.rs`, `appx.rs`; interface comum não confirmada |
| 72 | Operações privilegiadas tipadas | Parcial | `elevation.rs`; parser estrito testado, revisão completa do helper pendente |
| 73 | Roadmap | Parcial | Fases 1–14 documentadas em `docs/STATUS.md`; itens da fase 14 pendentes |
| 74 | Regras de implementação | Parcial | Builds/testes atuais passam; lacunas de testes e funcionalidade permanecem |
| 75 | Definição de pronto | Parcial | Vários recursos ainda sem validação completa de UI, cancelamento e permissões |
| 76 | Primeiro passo/continuidade | Parcial | Projeto funcional e compilável; escopo original ainda incompleto |

## Validação desta rodada

- Windows local: alvo `x86_64-pc-windows-msvc`. Não há evidência nesta rodada para Windows 11 ou ARM64.
- `npm run build`: passou.
- `cargo check --workspace --offline`: passou.
- `cargo clippy --workspace --all-targets --offline -- -D warnings`: passou.
- `cargo test --workspace --offline --no-run`: passou (compilação dos testes).
- `cargo test -p hdcleaner-core --lib --offline` com exclusão dos testes que gravam no volume do sistema e na área de transferência: 120 passaram, 1 ignorado.
- `cargo test -p test-fixtures --test cleaner_flow --offline`: 1 passou em diretórios temporários.

Os testes de fixtures que criam programas e entradas do Registro no perfil real, os instaladores e as operações de exclusão em dados reais exigem ambiente isolado antes de serem considerados validados. Não confundir build bem-sucedido com ausência de defeitos.

## Artefatos e smoke test

- `cargo build --release -p hdcleaner-cli --offline` e `npx tauri build`: passaram; MSI e NSIS/EXE x64 gerados.
- CLI `hdcleaner --version` mostrou `0.1.1`; `hdcleaner drives` listou as unidades locais.
- Varredura padrão de uma pasta controlada com 6 arquivos e 19,3 KB lógicos: processo completo em 282 ms de tempo externo; essa amostra não mede desempenho com milhões de arquivos.
- Executável gráfico abriu e permaneceu ativo por quatro segundos; foi encerrado após o smoke test. Instalação dos pacotes e interação visual não foram testadas em máquina isolada.
- MSI e NSIS/EXE aparecem como `NotSigned` em `Get-AuthenticodeSignature`; atualização automática permanece desativada.
