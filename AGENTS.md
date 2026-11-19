# Mriya WM — заметки для агентов

Правила из глобального `~/.pi/agent/AGENTS.md` действуют и здесь; ниже — только
то, что специфично для этого репозитория.

## Сборка и проверки

```sh
cargo build            # debug, для запуска вложенной сессии
cargo test             # 57 тестов, из них 1 ignored (загрузка всех системных тем)
cargo clippy --all-targets
cargo fmt
```

Запуск (вложенный бэкенд, окно внутри текущей сессии):

```sh
./target/debug/mriya-wm                 # ~/.config/mriya/config.kdl
./target/debug/mriya-wm --config ./my.kdl
./scripts/mriya-shot.sh /tmp/shot.png   # снимок ТОЛЬКО окна Mriya, не рабочего стола
```

Документация — `docs/architecture.md` (по-русски, по итерациям); пример конфига
— `docs/example-config.kdl`; конфиг панели — `examples/waybar/`.

## DRM-бэкенд: как проверять

Запуск на TTY (display manager стартует так же):

```sh
Ctrl+Alt+F2                                  # уйти с графической сессии
RUST_LOG=info,mriya_wm=debug mriya-wm --drm   # или ./target/release/mriya-wm --drm
Ctrl+Alt+F1                                  # вернуться
```

`Ctrl+Alt+F1` возвращает в сессию даже если композитор упал, поэтому проверять
только так, а не через greeter. Что смотреть в логе: `using this GPU as primary`,
`output configured` (коннектор и режим), затем отсутствие `failed to render an
output`. Ошибки `cannot open …` / `cannot become drm master` обычно означают, что
карта занята другой сессией (то есть запущено не с TTY) или нет прав logind.

Отладка на конкретной карте: `MRIYA_DRM_DEVICE=/dev/dri/card1`.

## Осторожно с чужими процессами

Никогда не убивать процессы пользователя по имени (`pkill -x waybar`,
`killall waybar`, `pkill -x firefox`): его waybar/niri/браузер работают в его
сессии, а не в тестовой. Тестовый процесс запускать с записью PID
(`… & echo $! > /tmp/waybar.pid`) и гасить только `kill $(cat /tmp/waybar.pid)`.
Паттерн `pkill -f` может совпасть с собственной командой `bash -c` и убить свою
же сессию.

## Релиз: GitHub и AUR

Репозиторий: <https://github.com/acidnik/mriya-wm> (public).
AUR: <https://aur.archlinux.org/packages/mriya-wm> (maintainer `acidnik`).

Раскладка, как в `parch`:

- `contrib/PKGBUILD` — **источник правды**, коммитится в основной репозиторий;
- `aur/` — локальный клон AUR-репозитория (`ssh://aur@aur.archlinux.org/mriya-wm.git`),
  в `.gitignore`, в основную историю не попадает; в нём лежат `PKGBUILD` и
  сгенерированный `.SRCINFO`.

### Быстрая сборка для разработки (без AUR)

Пока идёт работа над бэкендом, публиковать каждый коммит в AUR не нужно:

```sh
git pull
cd contrib && makepkg -si -p PKGBUILD.dev     # собирает из этого дерева
```

`contrib/PKGBUILD.dev` берёт `pkgver` из `Cargo.toml`, собирает `cargo build
--release --locked` в текущем дереве и ставит те же файлы, что и AUR-пакет.
`contrib/src`, `contrib/pkg` и `*.pkg.tar.zst` в `.gitignore`.

Если удобнее вообще без чек-аута — можно однажды залить в AUR пакет
`mriya-wm-git` (VCS-пакет собирает мастер-ветку), тогда хватит `paru -Syu`.
Пока его нет: dev-PKGBUILD проще и не требует обновлять пин на каждый коммит.

### Публикация в AUR (когда нужен релиз)

Порядок обновления:

1. Поднять версию: `pkgver` в `contrib/PKGBUILD` (и `version` в `Cargo.toml`).
   При смене `pkgver` — `pkgrel=1`, без смены версии — `pkgrel` +1.
2. Закоммитить и запушить в GitHub: `git push origin main`.
3. Взять SHA нового HEAD (`git rev-parse HEAD`) и вписать его в пин источника:
   `source=("$pkgname::git+$url.git#commit=<sha>")`. Пин обязателен — AUR не
   принимает плавающий `git+` без `#commit=`/`#tag=`.
4. Закоммитить `contrib/PKGBUILD` и запушить (пин должен указывать на коммит,
   в котором уже есть всё, что ставит `package()`, — `LICENSE`, `dist/`).
5. Собрать AUR-часть и проверить сборку **до** пуша:

   ```sh
   cp contrib/PKGBUILD aur/ && cd aur
   makepkg --printsrcinfo > .SRCINFO
   makepkg -f --noconfirm            # должно закончиться «Finished making»
   bsdtar -tf mriya-wm-*.pkg.tar.zst # /usr/bin/mriya-wm, mriya-session, wayland-sessions, LICENSE
   git add PKGBUILD .SRCINFO
   git commit -m "pkgrel=N: <что изменилось>"
   git push                          # ветка master, трекает origin/master
   ```

6. Проверить, что пакет живой: <https://aur.archlinux.org/packages/mriya-wm>.
   RPC (`/rpc/?v=5&type=info&arg[]=mriya-wm`) отстаёт на пару минут после пуша —
   пустой ответ сразу после заливки это нормально, страница пакета уже отдаёт 200.

Чего не забыть в `package()`:

- бинарник `target/release/mriya-wm` → `/usr/bin/mriya-wm`;
- `dist/mriya-session` → `/usr/bin/mriya-session`, `dist/mriya.desktop` →
  `/usr/share/wayland-sessions/mriya.desktop` (иначе пакет не появится в greeter);
- `LICENSE` → `/usr/share/licenses/mriya-wm/LICENSE`.

Про зависимости: `ldd` показывает почти ничего, потому что EGL/GLES и wayland
грузятся через `dlopen`; в `depends` они всё равно нужны (`mesa`, `wayland`,
`libxkbcommon`, `fontconfig`, `freetype2`). Курсорные темы — `optdepends`
(`xcursor-themes`).

Замечание про запуск: пакет ставится и работает как вложенный композитор, но
пункт в greeter заработает только после DRM/udev-бэкенда — чеклист в
`docs/architecture.md`, раздел «Следующие шаги».
