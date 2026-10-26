# Provider endpoint and recovery guidance

Post and Read Later already test their actual service endpoint before
installing credentials: Hermes requests the first letter with the bearer token;
Wallabag exchanges the password grant and requires both access and refresh
tokens. This change keeps the check but maps its result into the action the
owner can take:

- an authorization response says the token was rejected and should be renewed;
- no computer network says to reconnect before checking;
- timeout says nothing was installed and to retry when reachable;
- an unreachable/TLS failure says to check the HTTPS address and gives the
  exact private-CA trust command.

The server address is named in the error without printing the credential.
Post's token input is also bounded to a non-empty regular file no larger than
4 KiB. Three Post tests cover credential pinning, HTTPS/token input and the
error distinctions; three Read Later tests cover token parsing, encoding and
secret input. Strict all-target CLI Clippy passes.
