-- Which API a profile's endpoint speaks, for a client that can speak more
-- than one (Pi). Absent leaves the choice to the client's adapter.
ALTER TABLE endpoint_profiles ADD COLUMN api TEXT;
