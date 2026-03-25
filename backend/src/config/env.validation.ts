import * as Joi from 'joi';

export const validationSchema = Joi.object({
  NODE_ENV: Joi.string()
    .valid('development', 'production', 'test')
    .default('development'),
  PORT: Joi.number().default(3000),
  DATABASE_URL: Joi.string().required().description('PostgreSQL connection URL'),
  REDIS_URL: Joi.string().required().description('Redis connection URL'),
  SOROBAN_RPC_URL: Joi.string().required().description('Soroban RPC endpoint'),
  IPFS_GATEWAY: Joi.string().default('https://ipfs.io'),
  IPFS_PROJECT_ID: Joi.string().allow(''),
  IPFS_PROJECT_SECRET: Joi.string().allow(''),
  JWT_SECRET: Joi.string().min(32).required(),
  ADMIN_TOKEN: Joi.string().required(),
  LOG_LEVEL: Joi.string().default('info').valid('error', 'warn', 'log', 'verbose', 'debug'),
  CACHE_TTL_SECONDS: Joi.number().default(60).description('Cache TTL in seconds'),
});

