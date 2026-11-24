# Security

Report suspected vulnerabilities privately through the repository's security advisory channel when one is available. Include the affected version, impact and a minimal synthetic fixture. Avoid including original private files, passwords or output containing sensitive data.

The desktop contains a bundled UI, a local SQLite store and a raster adapter. There is no shell or HTTP integration. Processing runs under the current OS user in a local background thread. Independent process isolation and dedicated input sandboxing are pending. Limits and format validation reduce resource and processing risk while the supported codecs handle parsing. An output report identifies specific metadata probes and their limits. Visible sensitive content and anonymization require separate validation.
