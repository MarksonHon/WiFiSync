# AGENTS.md

## Code 
1. All code comments should be clear and concise, and write in English.
2. Code should be well-structured and follow consistent naming conventions.
3. This project needs building and running on OpenWrt systems, so make sure that everything is compatible with OpenWrt's environment and dependencies, especially when using system libraries(most importantly MUSL C standard library) and tools.

## Git
0. Always write commit messages in English.
1. Commit messages should be clear and descriptive, summarizing the changes made.
2. Use the imperative mood in commit messages (e.g., "Add feature" instead of "Added feature").
3. Keep commit messages concise, ideally under 50 characters for the subject line.
4. Include relevant issue or task references in the commit message when applicable.

## Documentation
0. Read all documentation files carefully to understand the project's structure and guidelines.
1. README.md should always be English, and README_${LANG}.md should be used for other languages, where `${LANG}` is the language code (e.g., `README_zh-cn.md` for Chinese Simplified), and all readme files only contains the basic information about the project.
2. All other documentation files should also be written in English, and translations should follow the same `${LANG}` convention if needed.
3. Sync the code to the `docs` directory, ensuring that all documentation is up-to-date with the latest code changes.

## LuCI
1. LuCI should not hardcode language, instead we should create languages packages for different languages and load them dynamically based on the user's preference.
2. LuCI should always be a web interface, it should not contain any code for sync network config directly; such functionality should be handled by backend services or agents.

## Backend Services
1. The service should be called 'wifisync', and follow the standard of the OpenWrt service conventions, including init scripts, configuration files, and logging practices.
2. Don't use any Systemd-specific features or configurations, as OpenWrt relies on its own init system.
3. Ensure that the backend service can communicate effectively with the LuCI interface and any other agents, using well-defined APIs or messaging protocols.

## CLI
1. Command line output must always be plain English: usage/help text, results, error messages and service log lines are never translated.
2. Localization only happens in the LuCI language packages; the CLI must not gain a language switch of its own.