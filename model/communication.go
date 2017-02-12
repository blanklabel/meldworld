package model

type ClientMessage struct {
	ModelType
	Msg    string `json:"msg"`
	Sender string `json:"sender"`
}

type ClientAction struct {
	// TODO: What can they do here?
}

type ModelType struct {
	MsgType string `json:"type"`
}
