package ect

import "runtime"

func DefaultConfig() Config {
	return normalizeConfig(Config{Level: 3, Workers: runtime.NumCPU()})
}

func normalizeConfig(cfg Config) Config {
	if cfg.Level < 1 {
		cfg.Level = 1
	}
	if cfg.Level > 9 {
		cfg.Level = 9
	}
	if cfg.Workers <= 0 {
		cfg.Workers = runtime.NumCPU()
	}
	if cfg.Workers <= 0 {
		cfg.Workers = 1
	}
	return cfg
}

func (c Config) logf(format string, args ...any) {
	if c.Quiet || c.Logf == nil {
		return
	}
	c.Logf(format, args...)
}
