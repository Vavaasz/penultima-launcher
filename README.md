# Game Launcher

Um launcher de jogos moderno e eficiente desenvolvido em Rust, oferecendo uma experiência fluida para gerenciar e executar jogos.

## 🚀 Características

- Interface gráfica moderna e responsiva
- Suporte a múltiplas instâncias de jogos
- Sistema de atualizações automáticas
- Gerenciamento de proxy configurável
- Cache inteligente para melhor performance
- Integração com a bandeja do sistema
- Interface de linha de comando (CLI)
- Suporte completo ao Windows

## 📋 Pré-requisitos

- Rust (versão 2021 ou superior)
- Windows 10/11
- Visual C++ Redistributable (para algumas dependências)

## 🛠️ Instalação

1. Clone o repositório:
```bash
git clone https://github.com/seu-usuario/game-launcher.git
cd game-launcher
```

2. Compile o projeto:
```bash
cargo build --release
```

3. O executável será gerado em `target/release/game-launcher.exe`

## 💻 Uso

### Interface Gráfica
Execute o arquivo `game-launcher.exe` para iniciar a interface gráfica.

### Linha de Comando
O launcher também suporta comandos via CLI. Use:
```bash
game-launcher.exe --help
```
para ver todas as opções disponíveis.

## 🔧 Configuração

O launcher utiliza um sistema de cache e diretórios de aplicativo para armazenar:
- Configurações do usuário
- Cache de atualizações
- Dados de instâncias
- Logs do sistema

## 📦 Dependências Principais

- tokio: Programação assíncrona
- eframe/egui: Interface gráfica
- reqwest: Requisições HTTP
- winapi/windows: Integração com Windows
- tray-icon: Ícone na bandeja do sistema
- single-instance: Controle de instância única

## 🔄 Sistema de Atualizações

O launcher inclui um sistema de atualizações automáticas que:
- Verifica novas versões periodicamente
- Baixa e instala atualizações em segundo plano

## 🤝 Contribuindo

Contribuições são bem-vindas! Por favor, siga estas etapas:

1. Faça um fork do projeto
2. Crie uma branch para sua feature (`git checkout -b feature/AmazingFeature`)
3. Commit suas mudanças (`git commit -m 'Add some AmazingFeature'`)
4. Push para a branch (`git push origin feature/AmazingFeature`)
5. Abra um Pull Request

## 📞 Suporte

Para suporte, abra uma issue no GitHub ou entre em contato através dos canais oficiais do projeto.

---
Desenvolvido com ❤️ usando Rust 