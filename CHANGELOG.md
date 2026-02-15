# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial public release
- TCP-based message protocol for secure CLI/HTTP proxying
- Policy-based authorization with allow/deny patterns
- Environment variable injection with policy precedence
- Audit logging with redaction support
- Automatic connection health monitoring and reconnection
- Rate limiting for HTTP services
- Systemd service templates and examples
- Comprehensive README and deployment guide

### Known Issues
- ⚠️ Early-stage software, not battle-tested
- Connection health check could be more robust
- Policy hot-reload not implemented (requires server restart)
- Limited to Linux + systemd

## Security Notices

This project is **NOT production-ready** and should be used with caution. Please read the security considerations in the README before deploying.

### Version 0.1.0 (Early Preview)
- Initial experimental release
- Core functionality working
- Not audited or tested in production
- API and configuration format may change

---

For security issues, please report privately instead of using the issue tracker.
