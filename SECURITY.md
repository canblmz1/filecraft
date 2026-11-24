# Security

Report suspected vulnerabilities privately through the repository's security advisory channel when one is available. Include the affected version, impact and a minimal synthetic fixture. Avoid including original private files, passwords or output containing sensitive data.

The desktop contains a bundled UI and local SQLite history. The native image and PDF adapters parse files without rendering or executing embedded actions. The optional MP4 adapter directly starts FFmpeg and FFprobe from the process PATH with restricted input options, using local staging files. Recipes cannot supply executables, arguments or URLs. The app processes files under the current OS user. Dedicated parser sandboxing and hard memory limits remain pending. Output reports identify the checks performed. Sharing sensitive material still requires checking visible content and metadata.
