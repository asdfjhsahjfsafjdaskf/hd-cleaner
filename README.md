# HD Cleaner

Analisador de espaço em disco e desinstalador avançado para Windows 10/11.
Núcleo em **Rust** (Tauri 2) e interface em **React + TypeScript**. Implementação
original, sem telemetria e sem nenhuma chamada de rede.

> Regra do projeto: **nada é simulado**. O que não está implementado aparece na
> interface marcado como "Ainda não implementado".

## O que ele faz

**Disco**

- Varredura de disco com dois motores: API do Windows (sem admin) e leitura direta
  da **MFT do NTFS** (com aprovação de administrador). No PC de desenvolvimento:
  1,26 milhão de arquivos em 3,5 s, e o total bate com o espaço usado informado
  pelo Windows.
- Árvore virtualizada, **treemap** desenhado em Canvas (layout calculado em Rust),
  busca com linguagem própria (`size:>1GB`, `modified:<30d`, `ext:mp4,mkv`, …),
  arquivos grandes, **duplicados** (tamanho → hash parcial BLAKE3 → hash completo)
  e comparação entre snapshots.

**Programas**

- Lista Win32, MSI e Microsoft Store/AppX com ícone, fabricante e **tamanho real**
  (pasta de instalação + AppData + ProgramData), separando programa, dados, cache e logs.
- Desinstalação com o desinstalador oficial e busca de **sobras** em três níveis,
  cada item com confiança, motivo e risco. Backup `.reg` antes de mexer no Registro.
- **Desinstalação forçada** (programa quebrado ou sem registro), **em lote** e
  **rápida** (remove sozinha só o que não deixa dúvida).

**Sistema**

- **Processos**: CPU, memória, usuário, fabricante; encerrar processo ou árvore,
  com verificação de identidade (PID + caminho) e proteção dos componentes do Windows.
- **Inicialização**: Run/RunOnce (usuário e máquina, 32/64 bits), pastas Inicializar,
  tarefas agendadas e serviços. Desabilitar usa o mesmo mecanismo do Windows
  (`StartupApproved`), então o Gerenciador de Tarefas mostra o mesmo estado.
  O **impacto na inicialização** vem do log de desempenho do próprio Windows.
- **Modo Alvo**: aponte para qualquer janela na tela e descubra o processo, o programa
  instalado e os itens de inicialização ligados a ela (inclui ícones da bandeja no Windows 10).
- **Limpeza**: temporários, caches, despejos, Lixeira, caches de aplicativos e de
  navegadores (Chrome, Edge, Brave, Vivaldi, Opera, Opera GX, Firefox) e itens recentes.

## Segurança

O projeto trata exclusão como a parte mais delicada do produto:

- **Plano → revisão → execução.** Todo item é reverificado antes de ser removido
  (tamanho, data e *fingerprint* volume+id do arquivo), o que fecha a janela TOCTOU.
- **Protection Engine** bloqueia pasta do Windows, boot/EFI, metadados NTFS, raízes de
  unidade e perfis inteiros; Program Files e ProgramData exigem confirmação extra.
- **Junções e links nunca são seguidos**; apagar um link não apaga o destino.
- **Menor privilégio**: o app roda sem administrador. O que precisa de elevação vai
  para um *helper* com superfície mínima (operações tipadas, revalidadas dentro dele),
  num único pedido de UAC.
- **Backup antes de remover**: `.reg` para Registro, cópia para arquivos de inicialização,
  XML para tarefas agendadas e cópia consistente do banco antes de mexer no Firefox.
- A limpeza **só remove o que a análise listou** e só se o item continuar igual.
- Tudo vai para um histórico local que também funciona como *journal* de recuperação.

## Início rápido

Pré-requisitos: Rust (MSVC), Node 20+, Visual Studio Build Tools e WebView2.

```powershell
npm install
npx tauri dev            # aplicativo em modo desenvolvimento
cargo test --workspace   # testes (os destrutivos usam pastas temporárias)
npx tauri build          # instaladores NSIS/MSI
```

## Linha de comando

```powershell
cargo build --release -p hdcleaner-cli
.\target\release\hdcleaner.exe drives
.\target\release\hdcleaner.exe scan C: --top 20
.\target\release\hdcleaner.exe search "C:\Users" "size:>1GB ext:iso,zip"
.\target\release\hdcleaner.exe duplicates "C:\Users" --min-size 20MB
.\target\release\hdcleaner.exe programs --sizes
.\target\release\hdcleaner.exe uninstall "Nome do programa" --confirm --remove-leftovers
.\target\release\hdcleaner.exe startup                  # o que inicia com o Windows
.\target\release\hdcleaner.exe startup --impact         # atrasos medidos pelo Windows
.\target\release\hdcleaner.exe processes --sort cpu
.\target\release\hdcleaner.exe cleanup                  # analisa (não remove nada)
.\target\release\hdcleaner.exe cleanup --run default --dry-run
.\target\release\hdcleaner.exe delete "C:\caminho\arquivo.tmp" --confirm
```

Nada destrutivo roda sem `--confirm`, e quase tudo aceita `--dry-run`.

## Estrutura

| Caminho | Conteúdo |
|---|---|
| `crates/hdcleaner-core` | Toda a lógica (varredura, busca, proteção, desinstalação, limpeza…), testável sem interface |
| `crates/hdcleaner-cli` | Binário `hdcleaner` |
| `crates/test-fixtures` | Programa falso usado nos testes de ponta a ponta |
| `src-tauri` | Aplicativo Tauri: comandos, estado e DTOs |
| `src` | Interface React + TypeScript |

Documentação: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) (arquitetura, modelo de
segurança e estado de cada funcionalidade) e [`docs/STATUS.md`](docs/STATUS.md)
(o que está pronto, o que falta e o que foi verificado em máquina real).

## Estado

Fases 1 a 9 prontas (disco, treemap, busca, duplicados, programas, desinstalação
normal/forçada/lote, processos, inicialização, Modo Alvo e limpeza). A seguir:
monitor de instalação, quarentena e página de Backups.

## Licença

MIT — veja [LICENSE](LICENSE).
